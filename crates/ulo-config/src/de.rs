//! The deserializer over the origin tree.
//!
//! It collects every missing, ill-typed and out-of-range value instead of stopping at the first.
//! Its error type renders `invalid_type`, `invalid_value` and `unknown_variant` from the kind of
//! the value found and never the value: serde's buffered content behind `#[serde(flatten)]` and
//! `#[serde(untagged)]` raises its errors through the outer deserializer's error type by those
//! same methods, so the rule holds there too, where no path is known to be a secret's.

use std::fmt;

use serde::de::{self, DeserializeOwned, Deserializer, Expected, Unexpected, Visitor};
use ulo::Secret;

use crate::config::Config;
use crate::error::ConfigError;
use crate::tree::Node;

/// The error of one deserialisation pass.
#[derive(Debug)]
pub(crate) struct DeError {
    pub(crate) message: String,
}

impl fmt::Display for DeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for DeError {}

impl de::Error for DeError {
    /// A leaf type's own message is printed as it wrote it, except under a secret's path.
    fn custom<T: fmt::Display>(msg: T) -> Self {
        let _ = msg;
        todo!()
    }

    fn invalid_type(unexp: Unexpected<'_>, exp: &dyn Expected) -> Self {
        let _ = (unexp, exp);
        todo!()
    }

    fn invalid_value(unexp: Unexpected<'_>, exp: &dyn Expected) -> Self {
        let _ = (unexp, exp);
        todo!()
    }

    fn unknown_variant(variant: &str, expected: &'static [&'static str]) -> Self {
        let _ = (variant, expected);
        todo!()
    }
}

/// A view of one node as a serde deserializer.
pub(crate) struct NodeDeserializer<'a> {
    pub(crate) node: &'a Node,
    pub(crate) path: String,
}

impl<'de> Deserializer<'de> for NodeDeserializer<'_> {
    type Error = DeError;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, DeError> {
        let _ = visitor;
        todo!()
    }

    /// A newtype named `Secret` is recorded with this node's path and origin and deserialised
    /// transparently.
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, DeError> {
        let _ = (name, visitor);
        todo!()
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf option
        unit unit_struct seq tuple tuple_struct map struct enum identifier ignored_any
    }
}

/// Deserialises the section at `path` as `T` inside the collector `ulo::__private::collect_secrets`
/// installs, one pass per section, so at most one leaf-type error is reported for it. Answers the
/// value or the error, and the secrets either path recorded.
pub(crate) fn section<T: DeserializeOwned>(
    config: &Config,
    path: &str,
) -> (Result<T, ConfigError>, Vec<Secret<String>>) {
    let _ = (config, path);
    todo!()
}
