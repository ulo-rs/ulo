use std::error::Error;
use std::fmt;
use std::time::Duration;

use ulo::{Module, ModuleDef, ModuleIdentity};

/// The module that reads `Many<dyn Check>`, binds [`Readiness`](crate::Readiness) and exports it.
///
/// Its `BeforeApplicationShutdown` hook turns readiness false, then waits the [`lag`](Self::lag),
/// bounded `After(lag + 1 s)` so the core's 10 s `hook_timeout` does not cut a longer lag.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthModule {
    lag: Duration,
}

impl HealthModule {
    pub fn new() -> Self {
        HealthModule { lag: Duration::ZERO }
    }

    /// How long the before-shutdown hook waits after turning readiness false, zero by default.
    pub fn lag(mut self, lag: Duration) -> Self {
        self.lag = lag;
        self
    }
}

impl Default for HealthModule {
    fn default() -> Self {
        HealthModule::new()
    }
}

impl Module for HealthModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = (m, self.lag);
        todo!()
    }
}

/// A check declared `Bound::After(Duration::ZERO)`, refused at `connect` where the health module
/// reads the collection.
#[derive(Debug)]
pub struct ZeroBound {
    pub(crate) check: String,
}

impl ZeroBound {
    /// The name of the check that declared the zero bound.
    pub fn check(&self) -> &str {
        &self.check
    }
}

impl fmt::Display for ZeroBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the health check `{}` declares a zero bound, which would mark it down before it ran", self.check)
    }
}

impl Error for ZeroBound {}
