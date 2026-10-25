//! Redaction (§9.3, §10.2). Every error the core did not create itself, from user code,
//! integrations or transports, passes through [`redact`] before it is stored in any core error
//! type, and what is stored is a [`Redacted`]. The scope is the error's origin, not a list of
//! variants.
//!
//! The function replaces every `Secret<_>` registered with the graph and, as a backstop, strips
//! the userinfo from anything shaped like a URL. On the `try_value` path the registry has
//! nothing to replace, since the secret sits inside the configuration that failed to load; the
//! strip is the only protection there, a backstop and not a guarantee.

use std::any::Any;
use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, PoisonError, RwLock};

use crate::timer::BoxError;

/// An error from outside the core, after the redaction function. Formatting writes the redacted
/// text and never the original; `source()` is `None`, so a reporter walking the chain stops at
/// the text too. The original is reached through [`downcast_ref`](Self::downcast_ref) and
/// [`into_inner`](Self::into_inner) alone, which is what lets an error handler map a
/// constructor's domain error to a status.
pub struct Redacted {
    inner: BoxError,
    text: String,
}

impl Redacted {
    /// The original, by type.
    pub fn downcast_ref<E: Error + 'static>(&self) -> Option<&E> {
        self.inner.downcast_ref::<E>()
    }

    pub fn into_inner(self) -> BoxError {
        self.inner
    }

    pub(crate) fn from_parts(inner: BoxError, text: String) -> Self {
        Redacted { inner, text }
    }

    /// The original, for the core's own walks of an error chain, which `source()` stops short of.
    pub(crate) fn original(&self) -> &(dyn Error + 'static) {
        &*self.inner
    }
}

impl fmt::Display for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl fmt::Debug for Redacted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Error for Redacted {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        None
    }
}

/// A value the redaction function replaces wherever it appears in an error this graph reports,
/// once registered: by `m.secret(&self.url)`, by `#[module]` for every `Secret<_>` field of a
/// configured module, or by binding it with `m.value(..)`.
///
/// `m.value(..)` registers a `Secret<String>` or a `Secret<&'static str>`. Any other `Secret<T>`
/// needs `m.secret(&s)` beside it, or its text stays visible in reports.
///
/// A secret shorter than four characters stays registered, and no message is matched against it:
/// a text that short is contained in most messages, and replacing it would mangle them without
/// hiding anything. `ulo-config` applies the same floor to the messages it checks.
///
/// `Debug` and `Display` never print the value. Equality and hashing are the value's, so a
/// configured module holding one keeps identity by value.
#[derive(Clone, PartialEq, Eq)]
pub struct Secret<T> {
    value: T,
}

impl<T> Secret<T> {
    pub fn new(value: T) -> Self {
        Secret { value }
    }

    pub fn expose(&self) -> &T {
        &self.value
    }

    pub fn into_inner(self) -> T {
        self.value
    }
}

impl<T: Hash> Hash for Secret<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret([redacted])")
    }
}

impl<T> fmt::Display for Secret<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

impl From<String> for Secret<String> {
    fn from(value: String) -> Self {
        Secret::new(value)
    }
}

impl From<&str> for Secret<String> {
    fn from(value: &str) -> Self {
        Secret::new(value.to_owned())
    }
}

thread_local! {
    /// The texts of the secrets deserialised on this thread while a collection is open, `None`
    /// outside one. A deserialisation is synchronous, so one thread-local scoped to the call holds
    /// exactly what that call produced.
    static COLLECTOR: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Runs `f` with a collection open on this thread and answers what it collected: the text of
/// every scalar a `Secret`'s inner value was read from. The collection open before, if any, is
/// restored afterwards, also when `f` unwinds, and receives nothing of this one's.
pub(crate) fn collecting<R>(f: impl FnOnce() -> R) -> (R, Vec<String>) {
    struct Restore(Option<Vec<String>>);

    impl Drop for Restore {
        fn drop(&mut self) {
            let previous = self.0.take();
            let _ = COLLECTOR.try_with(|c| *c.borrow_mut() = previous);
        }
    }

    let previous = COLLECTOR.try_with(|c| c.borrow_mut().replace(Vec::new())).ok().flatten();
    let restore = Restore(previous);
    let answer = f();
    let collected = COLLECTOR.try_with(|c| c.borrow_mut().take()).ok().flatten().unwrap_or_default();
    drop(restore);
    (answer, collected)
}

#[cfg(feature = "serde")]
mod serde_impl {
    //! `Secret<T>: Deserialize` and the capture that feeds the collector (integrations §2.3).
    //!
    //! The capture wraps the deserializer `T` reads from and records the text of every scalar it
    //! visits, so a `Secret<T>` is collected whatever `T` is, `Display` or not, and whatever
    //! deserializer it meets, serde's buffered content under `flatten` and `untagged` included.
    //! It descends through `Some` and newtypes; it records nothing inside a sequence, a map or an
    //! enum, and nothing under a request for an `Option`, whose flattened form serde answers
    //! through a visitor method that is not public API. A `bool` and a unit are not recorded: as
    //! redaction texts they would replace every `true` in every error.

    use std::fmt;
    use std::marker::PhantomData;

    use serde::de::{Deserialize, Deserializer, EnumAccess, Error, MapAccess, SeqAccess, Visitor};

    use super::{COLLECTOR, Secret};

    /// Read as a newtype named `"Secret"`, so `ulo-config`'s deserializer recognises it by that
    /// name and records it with its path and origin; any other deserializer reads the inner value
    /// as if the wrapper were absent. While `ulo-config`'s collection is open on the thread, the
    /// value's text is also recorded there, which is what registers a secret under
    /// `#[serde(flatten)]` and `#[serde(untagged)]`, where serde buffers the field and the name
    /// never reaches the deserializer.
    ///
    /// The limit: an error `T`'s own `Deserialize` raises through `Error::custom` carries `T`'s
    /// message, and such a message may quote its input. Outside `flatten` and `untagged`,
    /// `ulo-config` renders a problem under a secret's path from the path and the kind alone.
    /// Under them the secret is registered, through the collector, but its path is not known to
    /// be a secret's, so `ulo-config` checks the section's problems afterwards against the text
    /// of every secret the collector held and replaces a message that contains one. That holds
    /// when `T` read its input as a scalar before it failed, which is what put the text in the
    /// collector, and for a secret of four characters or more.
    ///
    /// `Secret` has no `Serialize`: writing one out through serde does not compile.
    impl<'de, T: Deserialize<'de>> Deserialize<'de> for Secret<T> {
        fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            deserializer.deserialize_newtype_struct("Secret", SecretVisitor(PhantomData))
        }
    }

    struct SecretVisitor<T>(PhantomData<fn() -> T>);

    impl<'de, T: Deserialize<'de>> Visitor<'de> for SecretVisitor<T> {
        type Value = Secret<T>;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a secret value")
        }

        fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<Secret<T>, D::Error> {
            let value = if collection_open() { T::deserialize(Capture(deserializer))? } else { T::deserialize(deserializer)? };
            Ok(Secret::new(value))
        }
    }

    fn collection_open() -> bool {
        COLLECTOR.try_with(|c| c.borrow().is_some()).unwrap_or(false)
    }

    /// An empty text is not recorded, as the registry ignores one.
    fn record(text: String) {
        if text.is_empty() {
            return;
        }
        let _ = COLLECTOR.try_with(|c| {
            if let Some(texts) = c.borrow_mut().as_mut() {
                texts.push(text);
            }
        });
    }

    struct Capture<D>(D);

    macro_rules! forward {
        ($($method:ident),* $(,)?) => {$(
            fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
                self.0.$method(Recording(visitor))
            }
        )*};
    }

    impl<'de, D: Deserializer<'de>> Deserializer<'de> for Capture<D> {
        type Error = D::Error;

        forward!(
            deserialize_any,
            deserialize_bool,
            deserialize_i8,
            deserialize_i16,
            deserialize_i32,
            deserialize_i64,
            deserialize_i128,
            deserialize_u8,
            deserialize_u16,
            deserialize_u32,
            deserialize_u64,
            deserialize_u128,
            deserialize_f32,
            deserialize_f64,
            deserialize_char,
            deserialize_str,
            deserialize_string,
            deserialize_bytes,
            deserialize_byte_buf,
            deserialize_unit,
            deserialize_seq,
            deserialize_map,
            deserialize_identifier,
            deserialize_ignored_any,
        );

        /// Unwrapped: see the module's documentation.
        fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Self::Error> {
            self.0.deserialize_option(visitor)
        }

        fn deserialize_unit_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, Self::Error> {
            self.0.deserialize_unit_struct(name, Recording(visitor))
        }

        fn deserialize_newtype_struct<V: Visitor<'de>>(self, name: &'static str, visitor: V) -> Result<V::Value, Self::Error> {
            self.0.deserialize_newtype_struct(name, Recording(visitor))
        }

        fn deserialize_tuple<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, Self::Error> {
            self.0.deserialize_tuple(len, Recording(visitor))
        }

        fn deserialize_tuple_struct<V: Visitor<'de>>(
            self,
            name: &'static str,
            len: usize,
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.deserialize_tuple_struct(name, len, Recording(visitor))
        }

        fn deserialize_struct<V: Visitor<'de>>(
            self,
            name: &'static str,
            fields: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.deserialize_struct(name, fields, Recording(visitor))
        }

        fn deserialize_enum<V: Visitor<'de>>(
            self,
            name: &'static str,
            variants: &'static [&'static str],
            visitor: V,
        ) -> Result<V::Value, Self::Error> {
            self.0.deserialize_enum(name, variants, Recording(visitor))
        }

        fn is_human_readable(&self) -> bool {
            self.0.is_human_readable()
        }
    }

    struct Recording<V>(V);

    macro_rules! record_display {
        ($($method:ident: $ty:ty),* $(,)?) => {$(
            fn $method<E: Error>(self, v: $ty) -> Result<Self::Value, E> {
                record(v.to_string());
                self.0.$method(v)
            }
        )*};
    }

    impl<'de, V: Visitor<'de>> Visitor<'de> for Recording<V> {
        type Value = V::Value;

        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            self.0.expecting(f)
        }

        record_display!(
            visit_i8: i8,
            visit_i16: i16,
            visit_i32: i32,
            visit_i64: i64,
            visit_i128: i128,
            visit_u8: u8,
            visit_u16: u16,
            visit_u32: u32,
            visit_u64: u64,
            visit_u128: u128,
            visit_f32: f32,
            visit_f64: f64,
            visit_char: char,
        );

        fn visit_bool<E: Error>(self, v: bool) -> Result<Self::Value, E> {
            self.0.visit_bool(v)
        }

        fn visit_str<E: Error>(self, v: &str) -> Result<Self::Value, E> {
            record(v.to_owned());
            self.0.visit_str(v)
        }

        fn visit_borrowed_str<E: Error>(self, v: &'de str) -> Result<Self::Value, E> {
            record(v.to_owned());
            self.0.visit_borrowed_str(v)
        }

        fn visit_string<E: Error>(self, v: String) -> Result<Self::Value, E> {
            record(v.clone());
            self.0.visit_string(v)
        }

        /// Recorded only as UTF-8: the registry replaces text.
        fn visit_bytes<E: Error>(self, v: &[u8]) -> Result<Self::Value, E> {
            record_bytes(v);
            self.0.visit_bytes(v)
        }

        fn visit_borrowed_bytes<E: Error>(self, v: &'de [u8]) -> Result<Self::Value, E> {
            record_bytes(v);
            self.0.visit_borrowed_bytes(v)
        }

        fn visit_byte_buf<E: Error>(self, v: Vec<u8>) -> Result<Self::Value, E> {
            record_bytes(&v);
            self.0.visit_byte_buf(v)
        }

        fn visit_none<E: Error>(self) -> Result<Self::Value, E> {
            self.0.visit_none()
        }

        fn visit_some<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
            self.0.visit_some(Capture(deserializer))
        }

        fn visit_unit<E: Error>(self) -> Result<Self::Value, E> {
            self.0.visit_unit()
        }

        fn visit_newtype_struct<D: Deserializer<'de>>(self, deserializer: D) -> Result<Self::Value, D::Error> {
            self.0.visit_newtype_struct(Capture(deserializer))
        }

        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
            self.0.visit_seq(seq)
        }

        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
            self.0.visit_map(map)
        }

        fn visit_enum<A: EnumAccess<'de>>(self, data: A) -> Result<Self::Value, A::Error> {
            self.0.visit_enum(data)
        }
    }

    fn record_bytes(v: &[u8]) {
        if let Ok(text) = std::str::from_utf8(v) {
            record(text.to_owned());
        }
    }
}

/// The secrets one graph registered, as the text the redaction function replaces. Per graph,
/// never process-wide, which would leak between tests.
#[derive(Clone, Default)]
pub(crate) struct SecretRegistry {
    pub(crate) texts: Vec<String>,
}

impl SecretRegistry {
    /// An empty text is ignored: it would match between every two characters.
    pub(crate) fn register(&mut self, text: String) {
        if !text.is_empty() && !self.texts.contains(&text) {
            self.texts.push(text);
        }
    }

    /// Every registered text of at least [`MIN_MATCHED`] characters replaced by the marker,
    /// longest first, so a secret containing another is replaced whole. One pass over the text, so
    /// a replacement is never matched again by a shorter secret.
    fn replace(&self, text: &str) -> String {
        let mut secrets: Vec<&str> =
            self.texts.iter().map(String::as_str).filter(|s| s.chars().count() >= MIN_MATCHED).collect();
        if secrets.is_empty() {
            return text.to_owned();
        }
        secrets.sort_by(|a, b| b.len().cmp(&a.len()));

        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        'scan: while !rest.is_empty() {
            for secret in &secrets {
                if let Some(after) = rest.strip_prefix(*secret) {
                    out.push_str(MARKER);
                    rest = after;
                    continue 'scan;
                }
            }
            let mut chars = rest.chars();
            if let Some(c) = chars.next() {
                out.push(c);
            }
            rest = chars.as_str();
        }
        out
    }
}

/// The secrets of the graph an app serves, for a value made before the graph exists that redacts
/// on its behalf: the app's runtime, which redacts the panic of a task spawned through it. The app
/// sets it from the graph `wire` built and again from each graph a `load` publishes.
#[derive(Clone, Default)]
pub(crate) struct Redactor(Arc<RwLock<Arc<SecretRegistry>>>);

impl Redactor {
    pub(crate) fn set(&self, secrets: &SecretRegistry) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(secrets.clone());
    }

    /// [`redact_panic`] with the secrets set last.
    pub(crate) fn panic(&self, payload: Box<dyn Any + Send>) -> Redacted {
        let secrets = Arc::clone(&*self.0.read().unwrap_or_else(PoisonError::into_inner));
        redact_panic(&secrets, payload)
    }
}

const MARKER: &str = "[redacted]";

/// The shortest secret, in characters, a message is matched against. A shorter one stays
/// registered; `ulo-config`'s scrub of its own messages uses the same floor.
const MIN_MATCHED: usize = 4;

/// The one function every outside error passes through before a core error type stores it.
///
/// The text is the error's message followed by each message down its `source()` chain, since
/// `Redacted::source()` answers `None` and a reporter can reach no further than the text. A link
/// whose message the text already holds is not repeated.
pub(crate) fn redact(secrets: &SecretRegistry, error: BoxError) -> Redacted {
    let text = scrub(secrets, &chain_text(&*error));
    Redacted::from_parts(error, text)
}

/// An outside error under a text the core wrote for it, such as a [`PrepareError`] written
/// against the whole startup report's names, scrubbed the same way.
///
/// [`PrepareError`]: crate::PrepareError
pub(crate) fn redact_as(secrets: &SecretRegistry, error: BoxError, text: String) -> Redacted {
    Redacted::from_parts(error, scrub(secrets, &text))
}

/// A caught panic payload, converted to a message and redacted the same way.
pub(crate) fn redact_panic(secrets: &SecretRegistry, payload: Box<dyn Any + Send>) -> Redacted {
    let message = if let Some(s) = payload.downcast_ref::<&'static str>() {
        (*s).to_owned()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "a panic whose payload is not a string".to_owned()
    };
    let text = scrub(secrets, &message);
    Redacted::from_parts(Box::new(PanicMessage(message)), text)
}

/// The backstop: `scheme://user:password@host` becomes `scheme://[redacted]@host`.
///
/// The authority ends where RFC 3986 ends it, at `/`, `?` or `#`, or at whitespace, a quote or
/// an angle bracket around a URL in prose; its last `@` ends the userinfo.
pub(crate) fn strip_userinfo(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        let (head, tail) = rest.split_at(at + "://".len());
        out.push_str(head);
        let end = tail
            .find(|c: char| matches!(c, '/' | '?' | '#' | '"' | '\'' | '`' | '<' | '>') || c.is_whitespace())
            .unwrap_or(tail.len());
        let (authority, after) = tail.split_at(end);
        match authority.rfind('@') {
            Some(i) => {
                out.push_str(MARKER);
                out.push_str(&authority[i..]);
            }
            None => out.push_str(authority),
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn scrub(secrets: &SecretRegistry, text: &str) -> String {
    strip_userinfo(&secrets.replace(text))
}

/// Bounded, in case a `source()` chain loops back on itself.
const MAX_CHAIN: usize = 32;

fn chain_text(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut next = error.source();
    let mut links = 0;
    while let Some(cause) = next {
        if links == MAX_CHAIN {
            break;
        }
        let message = cause.to_string();
        if !message.is_empty() && !text.contains(&message) {
            text.push_str(": ");
            text.push_str(&message);
        }
        next = cause.source();
        links += 1;
    }
    text
}

/// What a panic payload becomes inside a `Redacted`: the message, unredacted, reached only
/// through `into_inner`.
#[derive(Debug)]
struct PanicMessage(String);

impl fmt::Display for PanicMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for PanicMessage {}
