use ulo::{Module, ModuleDef, ModuleIdentity};

/// `/health/live`, `/health/ready` and `/health/started`, imported as a module beside
/// `ulo_health::HealthModule`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthHttp {
    pub(crate) live: String,
    pub(crate) ready: String,
    pub(crate) started: String,
    pub(crate) detail: bool,
}

impl HealthHttp {
    pub fn new() -> Self {
        HealthHttp {
            live: String::from("/health/live"),
            ready: String::from("/health/ready"),
            started: String::from("/health/started"),
            detail: false,
        }
    }

    pub fn live_path(mut self, path: impl Into<String>) -> Self {
        self.live = path.into();
        self
    }

    pub fn ready_path(mut self, path: impl Into<String>) -> Self {
        self.ready = path.into();
        self
    }

    pub fn started_path(mut self, path: impl Into<String>) -> Self {
        self.started = path.into();
        self
    }

    /// Adds the draft's `checks` object, each check with its redacted reason. Off by default, since
    /// it tells an outsider about internals.
    pub fn detail(mut self, on: bool) -> Self {
        self.detail = on;
        self
    }
}

impl Default for HealthHttp {
    fn default() -> Self {
        HealthHttp::new()
    }
}

impl Module for HealthHttp {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
        todo!()
    }
}
