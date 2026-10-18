use std::error::Error;
use std::fmt;

use ulo::BoxError;

/// The app's runtime is not one the store supports. Raised inside `ulo::Permanent` from the pool's
/// factory at `connect`, naming the module, the runtime it needs and the runtime it was given.
#[non_exhaustive]
#[derive(Debug)]
pub struct UnsupportedRuntime {
    pub module: &'static str,
    pub required: &'static str,
    /// The name `Spawn::name` answered; `"unknown"` for a runtime that declares none.
    pub given: &'static str,
}

impl UnsupportedRuntime {
    pub(crate) fn new(module: &'static str, required: &'static str, given: &'static str) -> Self {
        UnsupportedRuntime { module, required, given }
    }
}

impl fmt::Display for UnsupportedRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} needs the {} runtime, and the app was given {}", self.module, self.required, self.given)
    }
}

impl Error for UnsupportedRuntime {}

/// A migration failed, or the lock it runs under could not be taken within its bound. Raised inside
/// `ulo::Permanent` from the module's init hook: a misconfiguration, not retryable.
#[non_exhaustive]
#[derive(Debug)]
pub struct MigrationFailed {
    pub module: &'static str,
    pub source: BoxError,
}

impl MigrationFailed {
    pub(crate) fn new(module: &'static str, source: BoxError) -> Self {
        MigrationFailed { module, source }
    }
}

impl fmt::Display for MigrationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} migrations failed: {}", self.module, self.source)
    }
}

impl Error for MigrationFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}
