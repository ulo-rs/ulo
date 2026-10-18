use std::error::Error;
use std::fmt;

use crate::problems::Problems;

/// What a failed section read or `ConfigModule` records through `try_value`.
///
/// Its `Display` carries every problem, one per line with its path and origin, under a header line
/// of its own: `Redacted` flattens `source()` into one string, so a problem reachable only through
/// `source()` would be lost. The structured list stays reachable through
/// `Redacted::downcast_ref::<ConfigError>()`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ConfigError {
    pub(crate) header: String,
    pub(crate) problems: Problems,
}

impl ConfigError {
    pub fn problems(&self) -> &Problems {
        &self.problems
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let _ = f;
        todo!()
    }
}

impl Error for ConfigError {}
