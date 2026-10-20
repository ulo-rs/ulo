//! The deserializer over the origin tree.
//!
//! It collects every missing, ill-typed and out-of-range value instead of stopping at the first.
//! Its error type renders `invalid_type`, `invalid_value` and `unknown_variant` from the kind of
//! the value found and never the value: serde's buffered content behind `#[serde(flatten)]` and
//! `#[serde(untagged)]` raises its errors through the outer deserializer's error type by those
//! same methods, so the rule holds there too, where no path is known to be a secret's.
//!
//! A mismatch the deserializer detects itself is recorded and answered with a placeholder of the
//! type asked for, so the derive carries on and finds the next problem. A missing field cannot be
//! answered that way: the derive raises it after the fields are read, and ends the struct. It is
//! recorded, remembered as a patch, and the pass runs again with the field supplied as a
//! placeholder, until a pass finds nothing new. An error raised inside a leaf type's own
//! `Deserialize` has no placeholder, so it is recorded and ends the pass: at most one per section.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt;
use std::slice;

use serde::de::value::StrDeserializer;
use serde::de::{
    self, DeserializeOwned, DeserializeSeed, Deserializer, EnumAccess, Expected, IntoDeserializer, MapAccess, SeqAccess,
    Unexpected, VariantAccess, Visitor,
};
use ulo::Secret;

use crate::config::Config;
use crate::error::ConfigError;
use crate::problems::{Problem, Problems};
use crate::tree::{Node, Origin, Value, join_index, join_key};

/// A pass that never settles is cut here and reports what it found. Each pass adds one missing
/// field, so this bounds the fields of one configuration that can be reported missing together.
const MAX_PASSES: usize = 2048;

/// The shortest secret `scrub` matches against problem messages.
const MIN_SCRUBBED: usize = 4;

#[derive(Debug)]
enum Kind {
    /// Raised by `Error::custom`: a leaf type's own message, printed as it wrote it.
    Custom,
    /// Raised by this module from the kinds of the values involved, never the value.
    Type,
    /// `Error::missing_field`, which the table handler turns into a patch.
    Missing(String),
}

/// The error of one deserialisation pass.
#[derive(Debug)]
pub(crate) struct DeError {
    pub(crate) message: String,
    kind: Kind,
    /// Whether a problem for this error is already in the pass's list.
    recorded: bool,
}

impl DeError {
    fn new(kind: Kind, message: String) -> Self {
        DeError { message, kind, recorded: false }
    }

    /// Marks the error as already reported, so no ancestor records it again.
    fn silenced(mut self) -> Self {
        self.recorded = true;
        self
    }
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeError {}

fn describe(unexpected: &Unexpected<'_>) -> &'static str {
    match unexpected {
        Unexpected::Bool(_) => "a boolean",
        Unexpected::Unsigned(_) | Unexpected::Signed(_) => "an integer",
        Unexpected::Float(_) => "a float",
        Unexpected::Char(_) => "a character",
        Unexpected::Str(_) => "a string",
        Unexpected::Bytes(_) => "a byte string",
        Unexpected::Unit => "a unit value",
        Unexpected::Option => "an optional value",
        Unexpected::NewtypeStruct => "a newtype struct",
        Unexpected::Seq => "an array",
        Unexpected::Map => "a table",
        Unexpected::Enum => "an enum",
        Unexpected::UnitVariant => "a unit variant",
        Unexpected::NewtypeVariant => "a newtype variant",
        Unexpected::TupleVariant => "a tuple variant",
        Unexpected::StructVariant => "a struct variant",
        // `Other` carries a description the deserializer wrote, which serde's own defaults fill
        // with the value (`integer `123``).
        Unexpected::Other(_) => "an unsupported value",
    }
}

impl de::Error for DeError {
    /// A leaf type's own message is printed as it wrote it, except under a secret's path.
    fn custom<T: fmt::Display>(msg: T) -> Self {
        DeError::new(Kind::Custom, msg.to_string())
    }

    fn invalid_type(unexp: Unexpected<'_>, exp: &dyn Expected) -> Self {
        DeError::new(Kind::Type, format!("expected {exp}, found {}", describe(&unexp)))
    }

    fn invalid_value(unexp: Unexpected<'_>, exp: &dyn Expected) -> Self {
        DeError::new(Kind::Type, format!("invalid value: expected {exp}, found {}", describe(&unexp)))
    }

    fn unknown_variant(variant: &str, expected: &'static [&'static str]) -> Self {
        let _ = variant;
        let list: Vec<String> = expected.iter().map(|name| format!("`{name}`")).collect();
        let message = if list.is_empty() {
            "unknown variant, there are none".to_owned()
        } else {
            format!("unknown variant, expected one of {}", list.join(", "))
        };
        DeError::new(Kind::Type, message)
    }

    fn missing_field(field: &'static str) -> Self {
        DeError::new(Kind::Missing(field.to_owned()), format!("missing field `{field}`"))
    }
}

/// What one pass learns, shared by every deserializer of it.
struct Ctx {
    problems: RefCell<Problems>,
    secret_paths: RefCell<Vec<String>>,
    secret_texts: RefCell<Vec<String>>,
    consumed: RefCell<BTreeSet<String>>,
    /// Fields earlier passes found missing, as (table path, field).
    patches: Vec<(String, String)>,
    new_patch: RefCell<Option<NewPatch>>,
}

struct NewPatch {
    parent: String,
    field: String,
    /// The problem to report for the field, unless something above it was already reported.
    problem: Option<Problem>,
}

impl Ctx {
    fn new(patches: Vec<(String, String)>) -> Ctx {
        Ctx {
            problems: RefCell::new(Problems::new()),
            secret_paths: RefCell::new(Vec::new()),
            secret_texts: RefCell::new(Vec::new()),
            consumed: RefCell::new(BTreeSet::new()),
            patches,
            new_patch: RefCell::new(None),
        }
    }

    fn under_secret(&self, path: &str) -> bool {
        self.secret_paths.borrow().iter().any(|secret| {
            path == secret
                || path.strip_prefix(secret.as_str()).is_some_and(|rest| rest.starts_with('.') || rest.starts_with('['))
        })
    }
}

#[derive(Clone, Copy)]
enum Target<'a> {
    Node(&'a Node),
    /// No value at this path. `reported` is true when a problem for the absence, or for something
    /// above it, is already recorded, so the placeholder answers silently.
    Absent { reported: bool, origin: Option<&'a Origin> },
}

enum Leaf<'a> {
    Null,
    Bool(bool),
    Int(i128),
    Float(f64),
    Str(&'a str),
    Text(&'a str),
    Other,
}

/// A view of one node as a serde deserializer.
#[derive(Clone)]
pub(crate) struct NodeDeserializer<'a> {
    ctx: &'a Ctx,
    target: Target<'a>,
    path: String,
}

impl<'a> NodeDeserializer<'a> {
    fn origin(&self) -> Option<&'a Origin> {
        match self.target {
            Target::Node(node) => Some(&node.origin),
            Target::Absent { origin, .. } => origin,
        }
    }

    /// The kind of the value found, the only description of it an error carries.
    fn found(&self) -> &'static str {
        match self.target {
            Target::Node(node) => match &node.value {
                Value::Null => "null",
                Value::Bool(_) => "a boolean",
                Value::Integer(_) => "an integer",
                Value::Float(_) => "a float",
                Value::String(_) | Value::Text(_) => "a string",
                Value::Array(_) => "an array",
                Value::Table(_) => "a table",
            },
            Target::Absent { .. } => "nothing",
        }
    }

    fn consume(&self) {
        if matches!(self.target, Target::Node(_)) {
            self.ctx.consumed.borrow_mut().insert(self.path.clone());
        }
    }

    /// The scalar at this node, marking it read. Anything that is not a scalar is `Other`.
    fn leaf(&self) -> Leaf<'a> {
        let Target::Node(node) = self.target else { return Leaf::Other };
        let leaf = match &node.value {
            Value::Null => Leaf::Null,
            Value::Bool(flag) => Leaf::Bool(*flag),
            Value::Integer(number) => Leaf::Int(*number),
            Value::Float(number) => Leaf::Float(*number),
            Value::String(text) => Leaf::Str(text),
            Value::Text(text) => Leaf::Text(text),
            Value::Array(_) | Value::Table(_) => return Leaf::Other,
        };
        self.consume();
        leaf
    }

    fn record(&self, message: String) {
        self.record_in(message, false);
    }

    fn record_in(&self, message: String, flattened: bool) {
        let origin = self.origin().cloned();
        self.ctx.problems.borrow_mut().list.push(Problem { path: self.path.clone(), origin, message, flattened });
    }

    /// Records that this node is not what the caller asked for. A node that is absent is reported
    /// once, as missing, wherever the first read of it happens.
    fn expected(&self, expected: &str) {
        match self.target {
            Target::Node(_) => self.record(format!("expected {expected}, found {}", self.found())),
            Target::Absent { reported: false, .. } => self.record("missing required value".to_owned()),
            Target::Absent { reported: true, .. } => {}
        }
    }

    /// Records an error a visitor raised, once, at the node that handed it the value.
    fn done<T>(&self, result: Result<T, DeError>) -> Result<T, DeError> {
        result.map_err(|error| self.record_error(error))
    }

    fn record_error(&self, error: DeError) -> DeError {
        self.record_error_in(error, false)
    }

    fn record_error_in(&self, error: DeError, flattened: bool) -> DeError {
        if error.recorded {
            return error;
        }
        let message = if matches!(error.kind, Kind::Custom) && self.ctx.under_secret(&self.path) {
            // The type's message may quote the secret it was given.
            format!("invalid value, found {}", self.found())
        } else {
            error.message.clone()
        };
        self.record_in(message, flattened);
        error.silenced()
    }

    /// Answers a placeholder call whose failure is no new problem: the mismatch is recorded.
    fn settle<T>(&self, result: Result<T, DeError>) -> Result<T, DeError> {
        result.map_err(DeError::silenced)
    }

    fn note_secret(&self) {
        self.ctx.secret_paths.borrow_mut().push(self.path.clone());
        if let Target::Node(node) = self.target {
            collect_texts(node, &mut self.ctx.secret_texts.borrow_mut());
        }
    }

    fn int(&self) -> Option<i128> {
        match self.leaf() {
            Leaf::Int(number) => Some(number),
            Leaf::Text(text) => text.parse().ok(),
            _ => None,
        }
    }

    fn string<'de, V: Visitor<'de>>(&self, visitor: V, expected: &str) -> Result<V::Value, DeError> {
        match self.leaf() {
            Leaf::Str(text) | Leaf::Text(text) => self.done(visitor.visit_str(text)),
            _ => {
                self.expected(expected);
                self.settle(visitor.visit_str(""))
            }
        }
    }

    fn seq<'de, V: Visitor<'de>>(&self, visitor: V) -> Result<V::Value, DeError> {
        match self.target {
            Target::Node(Node { value: Value::Array(items), .. }) => {
                let access = SeqAcc { ctx: self.ctx, base: self.path.clone(), items: items.iter(), index: 0 };
                self.done(visitor.visit_seq(access))
            }
            _ => {
                self.expected("an array");
                let empty: &[Node] = &[];
                let access = SeqAcc { ctx: self.ctx, base: self.path.clone(), items: empty.iter(), index: 0 };
                self.settle(visitor.visit_seq(access))
            }
        }
    }

    fn table<'de, V: Visitor<'de>>(&self, visitor: V, via_map: bool) -> Result<V::Value, DeError> {
        let empty: &[(String, Node)] = &[];
        let (entries, silenced) = match self.target {
            Target::Node(Node { value: Value::Table(entries), .. }) => (entries.as_slice(), false),
            Target::Node(_) => {
                self.expected("a table");
                (empty, true)
            }
            Target::Absent { reported, .. } => (empty, reported),
        };
        let extras: Vec<String> = self
            .ctx
            .patches
            .iter()
            .filter(|(parent, field)| *parent == self.path && !entries.iter().any(|(key, _)| key == field))
            .map(|(_, field)| field.clone())
            .collect();
        let access = TableAcc {
            ctx: self.ctx,
            base: self.path.clone(),
            entries: entries.iter(),
            extras: extras.into_iter(),
            origin: self.origin(),
            pending: None,
        };
        let result = visitor.visit_map(access);
        self.table_result(result, silenced, via_map)
    }

    /// A missing field ends the derive's pass over the struct, so it becomes a patch the driver
    /// supplies on the next pass. Anything else a visitor raised is recorded where it arose.
    ///
    /// An error that comes out of the visitor unrecorded was raised below a deserializer that is
    /// not this one. Behind `#[serde(flatten)]`, which serde's derive reads through
    /// `deserialize_map`, that is the buffered content, and its error names no key; it is
    /// recorded against this table's path and origin and marked as inside a flattened section.
    fn table_result<T>(&self, result: Result<T, DeError>, silenced: bool, via_map: bool) -> Result<T, DeError> {
        match result {
            Err(DeError { kind: Kind::Missing(field), recorded: false, .. }) => {
                let problem = (!silenced).then(|| Problem {
                    path: join_key(&self.path, &field),
                    origin: self.origin().filter(|origin| !matches!(origin, Origin::Default)).cloned(),
                    message: "missing required field".to_owned(),
                    flattened: false,
                });
                let message = format!("missing field `{field}`");
                *self.ctx.new_patch.borrow_mut() = Some(NewPatch { parent: self.path.clone(), field: field.clone(), problem });
                Err(DeError::new(Kind::Missing(field), message).silenced())
            }
            other => other.map_err(|error| self.record_error_in(error, via_map)),
        }
    }
}

fn collect_texts(node: &Node, out: &mut Vec<String>) {
    match &node.value {
        Value::Null | Value::Bool(_) => {}
        Value::Integer(number) => out.push(number.to_string()),
        Value::Float(number) => out.push(number.to_string()),
        Value::String(text) | Value::Text(text) => {
            if !text.is_empty() {
                out.push(text.clone());
            }
        }
        Value::Array(items) => items.iter().for_each(|item| collect_texts(item, out)),
        Value::Table(entries) => entries.iter().for_each(|(_, child)| collect_texts(child, out)),
    }
}

fn visit_int<'de, V: Visitor<'de>>(number: i128, visitor: V) -> Result<V::Value, DeError> {
    if let Ok(narrow) = i64::try_from(number) {
        visitor.visit_i64(narrow)
    } else if let Ok(narrow) = u64::try_from(number) {
        visitor.visit_u64(narrow)
    } else {
        visitor.visit_i128(number)
    }
}

macro_rules! int_method {
    ($method:ident, $ty:ty, $name:literal) => {
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
            match self.int() {
                Some(number) if <$ty>::try_from(number).is_ok() => self.done(visit_int(number, visitor)),
                Some(_) => {
                    self.record(concat!("expected ", $name, ", found an integer out of range").to_owned());
                    self.settle(visitor.visit_u64(0))
                }
                None => {
                    self.expected($name);
                    self.settle(visitor.visit_u64(0))
                }
            }
        }
    };
}

impl<'de, 'a> Deserializer<'de> for NodeDeserializer<'a> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let Target::Node(node) = self.target else {
            self.expected("a value");
            return self.settle(visitor.visit_unit());
        };
        match &node.value {
            Value::Array(_) => self.seq(visitor),
            Value::Table(_) => self.table(visitor, false),
            _ => match self.leaf() {
                Leaf::Null => self.done(visitor.visit_unit()),
                Leaf::Bool(flag) => self.done(visitor.visit_bool(flag)),
                Leaf::Int(number) => self.done(visit_int(number, visitor)),
                Leaf::Float(number) => self.done(visitor.visit_f64(number)),
                Leaf::Str(text) | Leaf::Text(text) => self.done(visitor.visit_str(text)),
                Leaf::Other => self.done(visitor.visit_unit()),
            },
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let flag = match self.leaf() {
            Leaf::Bool(flag) => Some(flag),
            Leaf::Text(text) => text.parse::<bool>().ok(),
            _ => None,
        };
        match flag {
            Some(flag) => self.done(visitor.visit_bool(flag)),
            None => {
                self.expected("a boolean");
                self.settle(visitor.visit_bool(false))
            }
        }
    }

    int_method!(deserialize_i8, i8, "i8");
    int_method!(deserialize_i16, i16, "i16");
    int_method!(deserialize_i32, i32, "i32");
    int_method!(deserialize_i64, i64, "i64");
    int_method!(deserialize_i128, i128, "i128");
    int_method!(deserialize_u8, u8, "u8");
    int_method!(deserialize_u16, u16, "u16");
    int_method!(deserialize_u32, u32, "u32");
    int_method!(deserialize_u64, u64, "u64");
    int_method!(deserialize_u128, u128, "u128");

    fn deserialize_f32<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.float(visitor, "f32")
    }

    fn deserialize_f64<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.float(visitor, "f64")
    }

    fn deserialize_char<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.string(visitor, "a character")
    }

    fn deserialize_str<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.string(visitor, "a string")
    }

    fn deserialize_string<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.string(visitor, "a string")
    }

    fn deserialize_identifier<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.string(visitor, "a string")
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        if matches!(self.target, Target::Node(Node { value: Value::Array(_), .. })) {
            return self.seq(visitor);
        }
        match self.leaf() {
            Leaf::Str(text) | Leaf::Text(text) => self.done(visitor.visit_bytes(text.as_bytes())),
            _ => {
                self.expected("a byte string");
                self.settle(visitor.visit_bytes(&[]))
            }
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let target = self.target;
        match target {
            Target::Absent { .. } => self.done(visitor.visit_none()),
            Target::Node(node) if matches!(node.value, Value::Null) => {
                self.consume();
                self.done(visitor.visit_none())
            }
            Target::Node(_) => {
                let here = self.clone();
                let result = visitor.visit_some(self);
                here.done(result)
            }
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        match self.leaf() {
            Leaf::Null => self.done(visitor.visit_unit()),
            _ => {
                self.expected("null");
                self.settle(visitor.visit_unit())
            }
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, DeError> {
        self.deserialize_unit(visitor)
    }

    /// A newtype named `Secret` is recorded with this node's path and deserialised transparently.
    fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, DeError> {
        if name == "Secret" {
            self.note_secret();
        }
        let here = self.clone();
        let result = visitor.visit_newtype_struct(self);
        here.done(result)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.seq(visitor)
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, DeError> {
        self.seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _len: usize,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.seq(visitor)
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        self.table(visitor, true)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        self.table(visitor, false)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, DeError> {
        if let Target::Node(Node { value: Value::Table(entries), .. }) = self.target
            && let [(key, node)] = entries.as_slice()
        {
            let access = EnumAcc { ctx: self.ctx, path: join_key(&self.path, key), key, node };
            let result = visitor.visit_enum(access);
            return self.done(result);
        }
        match self.leaf() {
            Leaf::Str(text) | Leaf::Text(text) => {
                let access: StrDeserializer<'_, DeError> = text.into_deserializer();
                self.done(visitor.visit_enum(access))
            }
            _ => {
                self.expected("an enum");
                Err(DeError::new(Kind::Type, "no enum to read".to_owned()).silenced())
            }
        }
    }

    /// A value nothing reads is not consumed, so it is still reported as unread.
    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        visitor.visit_unit()
    }
}

impl NodeDeserializer<'_> {
    fn float<'de, V: Visitor<'de>>(&self, visitor: V, name: &str) -> Result<V::Value, DeError> {
        let number = match self.leaf() {
            Leaf::Float(number) => Some(number),
            Leaf::Int(number) => Some(number as f64),
            Leaf::Text(text) => text.parse::<f64>().ok(),
            _ => None,
        };
        match number {
            Some(number) => self.done(visitor.visit_f64(number)),
            None => {
                self.expected(name);
                self.settle(visitor.visit_f64(0.0))
            }
        }
    }
}

struct SeqAcc<'a> {
    ctx: &'a Ctx,
    base: String,
    items: slice::Iter<'a, Node>,
    index: usize,
}

impl<'de, 'a> SeqAccess<'de> for SeqAcc<'a> {
    type Error = DeError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, DeError> {
        let Some(node) = self.items.next() else { return Ok(None) };
        let path = join_index(&self.base, self.index);
        self.index += 1;
        seed.deserialize(NodeDeserializer { ctx: self.ctx, target: Target::Node(node), path }).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.items.len())
    }
}

struct TableAcc<'a> {
    ctx: &'a Ctx,
    base: String,
    entries: slice::Iter<'a, (String, Node)>,
    /// Fields earlier passes found missing, supplied as placeholders after the real entries.
    extras: std::vec::IntoIter<String>,
    origin: Option<&'a Origin>,
    pending: Option<(String, Target<'a>)>,
}

impl<'de, 'a> MapAccess<'de> for TableAcc<'a> {
    type Error = DeError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, DeError> {
        if let Some((key, node)) = self.entries.next() {
            let path = join_key(&self.base, key);
            self.pending = Some((path.clone(), Target::Node(node)));
            let text: StrDeserializer<'_, DeError> = key.as_str().into_deserializer();
            // A key the map's key type refuses, `abc` for a `u16`, is recorded at the key's own
            // path, so it never reaches `table_result` unrecorded and is not taken for a flattened
            // section's. Behind `flatten` keys are read as content and do not fail here.
            return seed
                .deserialize(text)
                .map(Some)
                .map_err(|error| NodeDeserializer { ctx: self.ctx, target: Target::Node(node), path }.record_error(error));
        }
        if let Some(key) = self.extras.next() {
            let target = Target::Absent { reported: true, origin: self.origin };
            self.pending = Some((join_key(&self.base, &key), target));
            let key: StrDeserializer<'_, DeError> = key.as_str().into_deserializer();
            return seed.deserialize(key).map(Some);
        }
        Ok(None)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, DeError> {
        let Some((path, target)) = self.pending.take() else {
            return Err(DeError::new(Kind::Type, "a table value was read before its key".to_owned()).silenced());
        };
        seed.deserialize(NodeDeserializer { ctx: self.ctx, target, path })
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.entries.len() + self.extras.len())
    }
}

/// An externally tagged enum written as a table of one entry.
struct EnumAcc<'a> {
    ctx: &'a Ctx,
    path: String,
    key: &'a str,
    node: &'a Node,
}

impl<'de, 'a> EnumAccess<'de> for EnumAcc<'a> {
    type Error = DeError;
    type Variant = VariantAcc<'a>;

    fn variant_seed<V: DeserializeSeed<'de>>(self, seed: V) -> Result<(V::Value, VariantAcc<'a>), DeError> {
        let key: StrDeserializer<'_, DeError> = self.key.into_deserializer();
        let value = seed.deserialize(key)?;
        Ok((value, VariantAcc { ctx: self.ctx, path: self.path, node: self.node }))
    }
}

struct VariantAcc<'a> {
    ctx: &'a Ctx,
    path: String,
    node: &'a Node,
}

impl VariantAcc<'_> {
    fn payload(&self) -> NodeDeserializer<'_> {
        NodeDeserializer { ctx: self.ctx, target: Target::Node(self.node), path: self.path.clone() }
    }
}

impl<'de, 'a> VariantAccess<'de> for VariantAcc<'a> {
    type Error = DeError;

    fn unit_variant(self) -> Result<(), DeError> {
        Ok(())
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, DeError> {
        seed.deserialize(self.payload())
    }

    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, DeError> {
        self.payload().deserialize_seq(visitor)
    }

    fn struct_variant<V: Visitor<'de>>(self, _fields: &'static [&'static str], visitor: V) -> Result<V::Value, DeError> {
        self.payload().deserialize_map(visitor)
    }
}

/// Second pass over the rendered problems: a message that contains the text of a collected secret
/// is replaced whole. Under `#[serde(flatten)]` and `#[serde(untagged)]` no path is known to be a
/// secret's, so a leaf type's `Error::custom` message reaches the problem list as written, and
/// this is the only place it is checked against the values the section's secrets held. It reads
/// the failures of the sources too, whose messages may quote the file's content.
///
/// A secret shorter than `MIN_SCRUBBED` characters is registered with the graph but not matched
/// here: a one- or two-character text is contained in most messages, and replacing them all would
/// hide every problem of the section without protecting anything a reader could recover.
fn scrub(problems: &mut Problems, secrets: &BTreeSet<String>) {
    for problem in &mut problems.list {
        if secrets
            .iter()
            .any(|text| text.chars().count() >= MIN_SCRUBBED && problem.message.contains(text.as_str()))
        {
            problem.message = "invalid value".to_owned();
        }
    }
}

/// Deserialises the section at `path` as `T` inside the collector `ulo::__private::collect_secrets`
/// installs, one pass per section, so at most one leaf-type error is reported for it. Answers the
/// value or the error, and the secrets either path recorded.
pub(crate) fn section<T: DeserializeOwned>(
    config: &Config,
    path: &str,
) -> (Result<T, ConfigError>, Vec<Secret<String>>) {
    let loaded = &*config.loaded;
    let root = loaded.tree.get(path);

    let mut patches: Vec<(String, String)> = Vec::new();
    let mut missing: Vec<Problem> = Vec::new();
    let mut texts: BTreeSet<String> = BTreeSet::new();
    let mut consumed: BTreeSet<String> = BTreeSet::new();
    let mut value: Option<T> = None;
    let mut found = Problems::new();

    for _ in 0..MAX_PASSES {
        let ctx = Ctx::new(patches.clone());
        let target = match root {
            Some(node) => Target::Node(node),
            None => Target::Absent { reported: false, origin: None },
        };
        let (result, collected) = ulo::__private::collect_secrets(|| {
            T::deserialize(NodeDeserializer { ctx: &ctx, target, path: path.to_owned() })
        });
        texts.extend(collected.texts().iter().cloned());
        texts.extend(ctx.secret_texts.take());
        consumed.extend(ctx.consumed.take());

        let patch = ctx.new_patch.take();
        let mut pass_problems = ctx.problems.take();
        match (result, patch) {
            (Err(_), Some(patch)) if !patches.iter().any(|(p, f)| *p == patch.parent && *f == patch.field) => {
                patches.push((patch.parent, patch.field));
                missing.extend(patch.problem);
            }
            (Ok(done), patch) => {
                missing.extend(patch.and_then(|patch| patch.problem));
                value = Some(done);
                found = pass_problems;
                break;
            }
            (Err(error), _) => {
                if !error.recorded {
                    let origin = root.map(|node| node.origin.clone());
                    pass_problems.list.push(Problem { path: path.to_owned(), origin, message: error.message, flattened: false });
                }
                found = pass_problems;
                break;
            }
        }
    }

    let mut problems = Problems::new();
    for failure in &loaded.failures {
        for problem in failure.problems.iter() {
            let origin = problem.origin.clone().unwrap_or_else(|| Origin::File {
                path: failure.source.as_str().into(),
                line: 0,
                column: 0,
            });
            problems.list.push(Problem { path: problem.path.clone(), origin: Some(origin), message: problem.message.clone(), flattened: false });
        }
    }
    problems.list.extend(found.list);
    problems.list.extend(missing);
    scrub(&mut problems, &texts);

    config.mark_consumed(consumed);
    let secrets: Vec<Secret<String>> = texts.into_iter().filter(|text| !text.is_empty()).map(Secret::new).collect();

    let outcome = match value {
        Some(value) if problems.is_empty() => Ok(value),
        _ => {
            if problems.is_empty() {
                problems.at(path, "the section could not be read");
            }
            let header = if path.is_empty() { "configuration is invalid".to_owned() } else { format!("section `{path}` is invalid") };
            Err(ConfigError { header, problems })
        }
    };
    (outcome, secrets)
}
