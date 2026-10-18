use serde::de::DeserializeOwned;
use ulo::{ModuleDef, Secret};

use crate::config::Config;
use crate::error::ConfigError;

/// One section of the configuration read as `T`, the read every integration's `from_config`
/// makes at its own registration without knowing the application's type.
///
/// The read deserialises the section at `path`, prefixes its problems with the path, and keeps the
/// secrets `T` holds. A failure is kept inside the value; [`register`](Self::register) hands it to
/// `try_value`, so `wire()` reports it with every other wiring error. Two sections are equal when
/// their paths and outcomes are, so a module holding one keeps its identity by value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Section<T> {
    pub(crate) path: String,
    pub(crate) outcome: Result<T, ConfigError>,
    pub(crate) secrets: Vec<Secret<String>>,
}

impl<T: DeserializeOwned> Section<T> {
    /// Reads the section at `path`, dotted: `"database"`, `"http.limits"`. The empty path is the
    /// whole configuration.
    pub fn read(config: &Config, path: &str) -> Section<T> {
        let _ = (config, path);
        todo!()
    }
}

impl<T> Section<T> {
    /// A section that already holds its value, for a module built in code and a test that has no
    /// `Config`. Nothing is read, so it carries no secrets: register any the value holds with
    /// `m.secret(..)`.
    pub fn from_value(path: impl Into<String>, value: T) -> Section<T> {
        Section { path: path.into(), outcome: Ok(value), secrets: Vec::new() }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// The value, or the error `register` would record.
    pub fn value(&self) -> Result<&T, &ConfigError> {
        self.outcome.as_ref()
    }

    /// Registers every secret the section held with `m.secret(..)`, so the graph's redaction
    /// replaces it in every error, and answers the value or the failure for `m.try_value(..)`.
    pub fn register(&self, m: &mut ModuleDef<'_>) -> Result<T, ConfigError>
    where
        T: Clone,
    {
        let _ = m;
        todo!()
    }
}
