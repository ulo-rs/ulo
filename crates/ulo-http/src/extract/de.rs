//! The deserializer behind `Path<T>`, `Query<T>` and `Form<T>`: a list of name and value pairs,
//! each value text parsed as the field's type asks.
//!
//! Serde names a struct's fields with `&'static str`, which is what lets a failure carry the
//! field's name as `ExtractError`'s `param`: in a query or a path a missing field is
//! `Missing { param: "page" }` and a value that does not parse `Malformed { param: "page", .. }`;
//! in a form body both are `Mismatched { param: "page", .. }`.
//!
//! In a query or a form, a struct's or a map's entries are the pairs grouped by name, each group in
//! the order its name first appears and its values in the order written: `?tag=a&tag=b` fills a
//! `Vec<String>` field with `["a", "b"]`, and a name written once fills it with one element. A
//! name written more than once into a field that is not a sequence fails naming the field, as
//! `serde_html_form` refuses it, rather than keeping the first or the last value. A sequence at the
//! top level, `Vec<(String, String)>`, takes every pair as written, repeats included.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::error::Error;
use std::fmt;

use percent_encoding::percent_decode;
use serde::de::{self, DeserializeOwned, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, SeqAccess, Visitor};
use ulo::AppHandle;
use ulo_transport::ExtractError;

use crate::cx::HttpCx;

/// Where the pairs come from, which decides what a top level other than a struct or a map means.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Source {
    /// A route's parameters: a tuple takes them in order and a scalar takes the only one.
    Path,
    /// A query string or a form body: a sequence takes `(name, value)` pairs.
    Form,
}

/// `T` from `pairs`.
pub(crate) fn from_pairs<T: DeserializeOwned>(pairs: &[(&str, &str)], source: Source) -> Result<T, ParamsError> {
    T::deserialize(Pairs { pairs, source })
}

/// `application/x-www-form-urlencoded` parsing as the WHATWG URL standard defines it: split at
/// `&`, empty pieces skipped, each split at its first `=`, `+` read as a space, then
/// percent-decoded and read as UTF-8 with U+FFFD in place of an invalid sequence.
pub(crate) fn parse_form(input: &[u8]) -> Vec<(String, String)> {
    input
        .split(|&byte| byte == b'&')
        .filter(|piece| !piece.is_empty())
        .map(|piece| match piece.iter().position(|&byte| byte == b'=') {
            Some(at) => (form_decode(&piece[..at]), form_decode(&piece[at + 1..])),
            None => (form_decode(piece), String::new()),
        })
        .collect()
}

fn form_decode(bytes: &[u8]) -> String {
    let spaced: Vec<u8> = bytes.iter().map(|&byte| if byte == b'+' { b' ' } else { byte }).collect();
    percent_decode(&spaced).decode_utf8_lossy().into_owned()
}

/// A failure deserializing the pairs, naming the field when serde names one.
#[derive(Debug)]
pub(crate) struct ParamsError {
    field: Option<&'static str>,
    missing: bool,
    message: String,
}

impl ParamsError {
    fn new(message: impl fmt::Display) -> Self {
        ParamsError { field: None, missing: false, message: message.to_string() }
    }

    /// Names `field` unless a deeper failure already named one.
    fn about(mut self, field: Option<&'static str>) -> Self {
        if self.field.is_none() {
            self.field = field;
        }
        self
    }

    /// The extraction failure: `Missing` for a field serde reported missing, `Malformed` otherwise,
    /// the error redacted through the app. `whole` names the parameter when no field is named.
    pub(crate) fn into_extract(self, whole: &'static str, cx: &HttpCx) -> ExtractError {
        self.into_extract_in(whole, cx.app())
    }

    /// [`into_extract`](Self::into_extract) with the error redacted through `app`.
    pub(crate) fn into_extract_in(self, whole: &'static str, app: &AppHandle) -> ExtractError {
        let param = self.field.unwrap_or(whole);
        if self.missing {
            ExtractError::Missing { param }
        } else {
            ExtractError::Malformed { param, source: app.redact(Box::new(self)) }
        }
    }

    /// The extraction failure for content: `Mismatched` whether serde reported a field missing or
    /// a value that does not parse, the content having decoded and not fit. A form body is
    /// content; a query string and a route's parameters are the request's target, which
    /// [`into_extract`](Self::into_extract) answers.
    pub(crate) fn into_mismatch(self, whole: &'static str, cx: &HttpCx) -> ExtractError {
        let param = self.field.unwrap_or(whole);
        ExtractError::Mismatched { param, source: cx.app().redact(Box::new(self)) }
    }
}

impl fmt::Display for ParamsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error for ParamsError {}

impl de::Error for ParamsError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        ParamsError::new(message)
    }

    fn missing_field(field: &'static str) -> Self {
        ParamsError { field: Some(field), missing: true, message: format!("missing field `{field}`") }
    }

    fn duplicate_field(field: &'static str) -> Self {
        ParamsError { field: Some(field), missing: false, message: format!("`{field}` is given more than once") }
    }
}

struct Pairs<'a> {
    pairs: &'a [(&'a str, &'a str)],
    source: Source,
}

impl<'a> Pairs<'a> {
    /// The one value a scalar or newtype takes: a route's only parameter.
    fn single(&self) -> Result<Value<'a>, ParamsError> {
        match (self.source, self.pairs) {
            (Source::Path, &[(_, text)]) => Ok(Value { text }),
            (Source::Path, pairs) => Err(ParamsError::new(format!("a single value on a route with {} parameters", pairs.len()))),
            (Source::Form, _) => Err(ParamsError::new("a query or form deserializes into named fields, not a single value")),
        }
    }

    /// The entries a struct or a map reads: a route's parameters one by one, a query's or a
    /// form's grouped by name.
    fn entries<'de, V: Visitor<'de>>(self, fields: &'static [&'static str], visitor: V) -> Result<V::Value, ParamsError> {
        let groups = match self.source {
            Source::Path => Groups::single(self.pairs),
            Source::Form => Groups::by_name(self.pairs),
        };
        visitor.visit_map(Entries { pairs: self.pairs, groups: &groups, at: 0, fields, value: None })
    }
}

/// The pairs grouped by name without copying them: `heads` holds each name's first pair and its
/// count, and `next` chains each pair to the next one under its name. Building it is one pass with
/// one hash lookup per pair, its memory linear in the number of pairs however the names repeat.
struct Groups {
    /// `(first pair, pairs under the name)`, in the order the names first appear.
    heads: Vec<(usize, usize)>,
    /// For each pair, the index of the next pair under the same name; `NONE` for the last.
    next: Vec<usize>,
}

const NONE: usize = usize::MAX;

impl Groups {
    /// Each pair a group of its own: a route's parameter names are distinct.
    fn single(pairs: &[(&str, &str)]) -> Groups {
        Groups { heads: (0..pairs.len()).map(|at| (at, 1)).collect(), next: vec![NONE; pairs.len()] }
    }

    fn by_name(pairs: &[(&str, &str)]) -> Groups {
        let mut heads: Vec<(usize, usize)> = Vec::new();
        let mut next = vec![NONE; pairs.len()];
        // Per name: its group in `heads` and the last pair chained so far.
        let mut seen: HashMap<&str, (usize, usize)> = HashMap::new();
        for (at, &(name, _)) in pairs.iter().enumerate() {
            match seen.entry(name) {
                Entry::Occupied(mut entry) => {
                    let (group, last) = entry.get_mut();
                    next[*last] = at;
                    *last = at;
                    heads[*group].1 += 1;
                }
                Entry::Vacant(entry) => {
                    entry.insert((heads.len(), at));
                    heads.push((at, 1));
                }
            }
        }
        Groups { heads, next }
    }
}

macro_rules! single_value {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
            self.single()?.$method(visitor)
        }
    )*};
}

impl<'de, 'a> Deserializer<'de> for Pairs<'a> {
    type Error = ParamsError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        match (self.source, self.pairs) {
            (Source::Path, &[(_, text)]) => Value { text }.deserialize_any(visitor),
            _ => self.deserialize_map(visitor),
        }
    }

    fn deserialize_map<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        self.entries(&[], visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        self.entries(fields, visitor)
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        match self.source {
            Source::Path => visitor.visit_seq(Values { iter: self.pairs.iter() }),
            Source::Form => visitor.visit_seq(PairSeq { iter: self.pairs.iter() }),
        }
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, ParamsError> {
        if self.source == Source::Path && len != self.pairs.len() {
            return Err(ParamsError::new(format!("a tuple of {len} on a route with {} parameters", self.pairs.len())));
        }
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, _name: &'static str, len: usize, visitor: V) -> Result<V::Value, ParamsError> {
        self.deserialize_tuple(len, visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        self.single()?.deserialize_enum(name, variants, visitor)
    }

    single_value! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_f32 deserialize_f64 deserialize_char deserialize_str deserialize_string
        deserialize_bytes deserialize_byte_buf deserialize_identifier
    }
}

/// One value's text, parsed as the visitor asks.
#[derive(Clone, Copy)]
struct Value<'a> {
    text: &'a str,
}

macro_rules! parse_value {
    ($($method:ident => $visit:ident: $ty:ty,)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
            match self.text.parse::<$ty>() {
                Ok(value) => visitor.$visit(value),
                Err(_) => Err(ParamsError::new(concat!("the value is not a valid `", stringify!($ty), "`"))),
            }
        }
    )*};
}

impl<'de, 'a> Deserializer<'de> for Value<'a> {
    type Error = ParamsError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_str(self.text)
    }

    parse_value! {
        deserialize_bool => visit_bool: bool,
        deserialize_i8 => visit_i8: i8,
        deserialize_i16 => visit_i16: i16,
        deserialize_i32 => visit_i32: i32,
        deserialize_i64 => visit_i64: i64,
        deserialize_i128 => visit_i128: i128,
        deserialize_u8 => visit_u8: u8,
        deserialize_u16 => visit_u16: u16,
        deserialize_u32 => visit_u32: u32,
        deserialize_u64 => visit_u64: u64,
        deserialize_u128 => visit_u128: u128,
        deserialize_f32 => visit_f32: f32,
        deserialize_f64 => visit_f64: f64,
        deserialize_char => visit_char: char,
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_some(self)
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        IntoDeserializer::<'de, ParamsError>::into_deserializer(self.text).deserialize_enum(name, variants, visitor)
    }

    serde::forward_to_deserialize_any! {
        str string bytes byte_buf identifier unit_struct seq tuple tuple_struct map struct ignored_any
    }
}

/// A struct's or a map's entries, one per name; `fields` is the struct's field names, empty for a
/// map.
struct Entries<'a> {
    pairs: &'a [(&'a str, &'a str)],
    groups: &'a Groups,
    /// The next group in `groups.heads`.
    at: usize,
    fields: &'static [&'static str],
    /// The values of the key just read, with the field name serde gave for it.
    value: Option<(Named<'a>, Option<&'static str>)>,
}

impl<'de, 'a> MapAccess<'de> for Entries<'a> {
    type Error = ParamsError;

    fn next_key_seed<K: DeserializeSeed<'de>>(&mut self, seed: K) -> Result<Option<K::Value>, ParamsError> {
        let Some(&(first, count)) = self.groups.heads.get(self.at) else {
            return Ok(None);
        };
        self.at += 1;
        let key = self.pairs[first].0;
        let field = self.fields.iter().copied().find(|field| *field == key);
        self.value = Some((Named { pairs: self.pairs, next: &self.groups.next, first, count }, field));
        seed.deserialize(IntoDeserializer::<'de, ParamsError>::into_deserializer(key)).map(Some).map_err(|error| error.about(field))
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, ParamsError> {
        let Some((values, field)) = self.value.take() else {
            return Err(ParamsError::new("a value was asked for before its key"));
        };
        seed.deserialize(values).map_err(|error| error.about(field))
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.groups.heads.len() - self.at)
    }
}

/// The values written under one name: a sequence reads them all, in the order written, and any
/// other type reads the one value, refusing a name written more than once.
#[derive(Clone, Copy)]
struct Named<'a> {
    pairs: &'a [(&'a str, &'a str)],
    next: &'a [usize],
    first: usize,
    count: usize,
}

impl<'a> Named<'a> {
    fn one(self) -> Result<Value<'a>, ParamsError> {
        if self.count == 1 {
            return Ok(Value { text: self.pairs[self.first].1 });
        }
        let name = self.pairs[self.first].0;
        Err(ParamsError::new(format!(
            "`{name}` is given {} times, and only a sequence such as a `Vec` takes more than one value",
            self.count
        )))
    }

    fn values(self) -> NamedValues<'a> {
        NamedValues { pairs: self.pairs, next: self.next, at: self.first, left: self.count }
    }
}

macro_rules! one_value {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
            self.one()?.$method(visitor)
        }
    )*};
}

impl<'de, 'a> Deserializer<'de> for Named<'a> {
    type Error = ParamsError;

    /// One value as text, several as a sequence: what a flattened struct buffers a value as.
    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        match self.count {
            1 => self.one()?.deserialize_any(visitor),
            _ => visitor.visit_seq(self.values()),
        }
    }

    fn deserialize_seq<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_seq(self.values())
    }

    fn deserialize_tuple<V: Visitor<'de>>(self, _len: usize, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_seq(self.values())
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_some(self)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(self, _name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        visitor.visit_unit()
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, ParamsError> {
        self.one()?.deserialize_unit_struct(name, visitor)
    }

    fn deserialize_tuple_struct<V: Visitor<'de>>(self, name: &'static str, len: usize, visitor: V) -> Result<V::Value, ParamsError> {
        self.one()?.deserialize_tuple_struct(name, len, visitor)
    }

    fn deserialize_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        self.one()?.deserialize_struct(name, fields, visitor)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, ParamsError> {
        self.one()?.deserialize_enum(name, variants, visitor)
    }

    one_value! {
        deserialize_bool deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_f32 deserialize_f64 deserialize_char deserialize_str deserialize_string
        deserialize_bytes deserialize_byte_buf deserialize_identifier deserialize_unit deserialize_map
    }
}

/// The values under one name, in the order written.
struct NamedValues<'a> {
    pairs: &'a [(&'a str, &'a str)],
    next: &'a [usize],
    at: usize,
    left: usize,
}

impl<'de, 'a> SeqAccess<'de> for NamedValues<'a> {
    type Error = ParamsError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, ParamsError> {
        if self.left == 0 {
            return Ok(None);
        }
        let text = self.pairs[self.at].1;
        self.left -= 1;
        self.at = self.next[self.at];
        seed.deserialize(Value { text }).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.left)
    }
}

/// A route's parameter values, in the order the pattern names them.
struct Values<'a> {
    iter: std::slice::Iter<'a, (&'a str, &'a str)>,
}

impl<'de, 'a> SeqAccess<'de> for Values<'a> {
    type Error = ParamsError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, ParamsError> {
        match self.iter.next() {
            Some(&(_, text)) => seed.deserialize(Value { text }).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

/// A query's or a form's pairs as a sequence, each a `(name, value)` tuple.
struct PairSeq<'a> {
    iter: std::slice::Iter<'a, (&'a str, &'a str)>,
}

impl<'de, 'a> SeqAccess<'de> for PairSeq<'a> {
    type Error = ParamsError;

    fn next_element_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<Option<T::Value>, ParamsError> {
        match self.iter.next() {
            Some(&(name, text)) => seed.deserialize(Pair { name, text }).map(Some),
            None => Ok(None),
        }
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.iter.len())
    }
}

struct Pair<'a> {
    name: &'a str,
    text: &'a str,
}

impl<'de, 'a> Deserializer<'de> for Pair<'a> {
    type Error = ParamsError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, ParamsError> {
        let texts = [("", self.name), ("", self.text)];
        visitor.visit_seq(Values { iter: texts.iter() })
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option unit
        unit_struct newtype_struct seq tuple tuple_struct map struct enum identifier ignored_any
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, HashMap};

    use serde::Deserialize;
    use serde::de::DeserializeOwned;

    use super::{Source, from_pairs, parse_form};

    /// `T` from a query string as `Query<T>` reads it, or the failure as `field|message`.
    fn query<T: DeserializeOwned>(text: &str) -> Result<T, String> {
        let decoded = parse_form(text.as_bytes());
        let pairs: Vec<(&str, &str)> = decoded.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
        from_pairs::<T>(&pairs, Source::Form).map_err(|error| format!("{}|{}", error.field.unwrap_or("-"), error.message))
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Tags {
        tag: Vec<String>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Numbers {
        n: Vec<u32>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct MaybeTags {
        tag: Option<Vec<String>>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Page {
        page: u32,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Mixed {
        page: u32,
        tag: Vec<String>,
        q: Option<String>,
    }

    #[test]
    fn a_repeated_key_fills_a_vec_in_the_order_written() {
        assert_eq!(query::<Tags>("tag=b&tag=a&tag=c"), Ok(Tags { tag: vec!["b".into(), "a".into(), "c".into()] }));
    }

    #[test]
    fn a_repeated_key_fills_a_vec_of_a_parsed_type_and_a_value_that_does_not_parse_names_the_field() {
        assert_eq!(query::<Numbers>("n=3&n=1"), Ok(Numbers { n: vec![3, 1] }));
        assert_eq!(query::<Numbers>("n=3&n=x"), Err("n|the value is not a valid `u32`".to_owned()));
    }

    #[test]
    fn a_key_written_once_fills_a_vec_with_one_element() {
        assert_eq!(query::<Tags>("tag=a"), Ok(Tags { tag: vec!["a".into()] }));
    }

    #[test]
    fn an_optional_vec_is_none_when_absent_and_holds_every_value_when_present() {
        assert_eq!(query::<MaybeTags>(""), Ok(MaybeTags { tag: None }));
        assert_eq!(query::<MaybeTags>("tag=a"), Ok(MaybeTags { tag: Some(vec!["a".into()]) }));
        assert_eq!(query::<MaybeTags>("tag=a&tag=b"), Ok(MaybeTags { tag: Some(vec!["a".into(), "b".into()]) }));
    }

    #[test]
    fn a_repeated_key_into_a_scalar_fails_naming_the_field_and_the_count() {
        assert_eq!(
            query::<Page>("page=1&page=2"),
            Err("page|`page` is given 2 times, and only a sequence such as a `Vec` takes more than one value".to_owned())
        );
        assert!(query::<Option<Page>>("page=1&page=2").is_err(), "an `Option` around the struct refuses it too");
    }

    #[test]
    fn a_repeated_key_into_a_map_of_scalars_fails_naming_the_key_and_a_map_of_vecs_groups_it() {
        let refused = query::<HashMap<String, String>>("a=1&a=2").expect_err("a map of scalars takes one value per key");
        assert!(refused.contains("`a` is given 2 times"), "{refused}");
        let grouped = BTreeMap::from([("a".to_owned(), vec!["1".to_owned(), "2".to_owned()]), ("b".to_owned(), vec!["3".to_owned()])]);
        assert_eq!(query::<BTreeMap<String, Vec<String>>>("a=1&b=3&a=2"), Ok(grouped));
    }

    #[test]
    fn keys_and_values_are_percent_decoded_and_plus_is_a_space_before_grouping() {
        let tags = Tags { tag: vec!["a b".into(), "c+d".into(), "\u{2713}".into()] };
        assert_eq!(query::<Tags>("t%61g=a+b&tag=c%2Bd&ta%67=%E2%9C%93"), Ok(tags));
        assert_eq!(query::<HashMap<String, String>>("a+b=1"), Ok(HashMap::from([("a b".to_owned(), "1".to_owned())])));
    }

    #[test]
    fn an_empty_value_and_a_key_without_equals_are_empty_strings() {
        assert_eq!(query::<Tags>("tag=&tag"), Ok(Tags { tag: vec![String::new(), String::new()] }));
    }

    #[test]
    fn repeats_of_one_key_interleaved_with_others_keep_every_field() {
        let mixed = Mixed { page: 2, tag: vec!["a".into(), "b".into()], q: Some("x".into()) };
        assert_eq!(query::<Mixed>("tag=a&page=2&tag=b&q=x"), Ok(mixed));
    }

    #[test]
    fn a_top_level_sequence_takes_every_pair_as_written() {
        let pairs = vec![("b".into(), "1".into()), ("a".into(), "2".into()), ("b".into(), "3".into())];
        assert_eq!(query::<Vec<(String, String)>>("b=1&a=2&b=3"), Ok(pairs));
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Nested {
        inner: Page,
    }

    #[test]
    fn a_struct_valued_field_is_refused_written_once_or_repeated() {
        assert!(query::<Nested>("inner=1").is_err());
        assert!(query::<Nested>("inner=1&inner=2").is_err());
    }

    #[derive(Debug, Deserialize, PartialEq)]
    struct Flat {
        page: u32,
        #[serde(flatten)]
        rest: HashMap<String, Vec<String>>,
    }

    #[test]
    fn a_flattened_map_takes_a_repeated_key_as_a_sequence() {
        let flat = query::<Flat>("page=1&tag=a&tag=b").expect("a flattened map of sequences");
        assert_eq!((flat.page, flat.rest.get("tag")), (1, Some(&vec!["a".to_owned(), "b".to_owned()])));
    }

    /// 100,000 pairs under one name and 100,000 under distinct names each group in one pass: a
    /// scan of the groups so far per pair would make five billion comparisons on the second.
    #[test]
    fn a_hostile_query_of_many_pairs_groups_in_one_pass() {
        let repeated = "n=1&".repeat(100_000);
        assert_eq!(query::<Numbers>(&repeated).map(|numbers| numbers.n.len()), Ok(100_000));
        assert!(query::<Page>(&format!("page=1&{}", "page=2&".repeat(100_000))).is_err());
        let distinct: String = (0..100_000).map(|n| format!("k{n}=1&")).collect();
        assert_eq!(query::<HashMap<String, String>>(&distinct).map(|map| map.len()), Ok(100_000));
    }
}
