use std::sync::Arc;

use serde::de::DeserializeOwned;
use ulo::{Module, ModuleDef, ModuleIdentity, ModuleSettings};

use crate::config::Config;
use crate::error::ConfigError;
use crate::problems::{Problem, Problems};
use crate::section::Section;

type Validate<T> = dyn Fn(&T, &mut Problems) + Send + Sync;

/// Registers the whole configuration as `T` through `try_value`, and exports it.
///
/// The `Err` it records carries every source failure, every value missing, ill-typed or invalid in
/// `T`, and every finding of [`validate`](Self::validate). At `connect` its init hook logs at
/// `warn` each leaf no section read, with its origin, or refuses under
/// [`deny_unread`](Self::deny_unread); `wire()` cannot do either, since the core runs no hook after
/// every module has registered and a lazily loaded module may read a key later.
pub struct ConfigModule<T> {
    section: Section<T>,
    config: Config,
    validate: Option<Arc<Validate<T>>>,
    deny_unread: bool,
}

impl<T: DeserializeOwned> ConfigModule<T> {
    pub fn new(config: &Config) -> Self {
        ConfigModule {
            section: Section::read(config, ""),
            config: config.clone(),
            validate: None,
            deny_unread: false,
        }
    }
}

impl<T> ConfigModule<T> {
    /// Runs `check` on the loaded value; every `p.at(path, message)` it records joins the report.
    pub fn validate(mut self, check: impl Fn(&T, &mut Problems) + Send + Sync + 'static) -> Self {
        self.validate = Some(Arc::new(check));
        self
    }

    /// Refuses at `connect`, instead of warning, when a key was supplied that no section read.
    pub fn deny_unread(mut self) -> Self {
        self.deny_unread = true;
        self
    }
}

impl<T> Module for ConfigModule<T>
where
    T: DeserializeOwned + Clone + Send + Sync + 'static,
{
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn settings(&self) -> ModuleSettings {
        ModuleSettings::new().with("deny_unread", self.deny_unread)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let outcome = self.section.register(m).and_then(|value| match &self.validate {
            Some(check) => {
                let mut problems = Problems::new();
                check(&value, &mut problems);
                if problems.is_empty() {
                    Ok(value)
                } else {
                    Err(ConfigError { header: String::from("configuration is invalid"), problems })
                }
            }
            None => Ok(value),
        });
        m.try_value(outcome);
        m.export::<T>();

        let config = self.config.clone();
        let deny = self.deny_unread;
        m.on_init(move || {
            let config = config.clone();
            async move { check_unread(&config, deny) }
        });
    }
}

/// Runs at `connect`, after every module has registered and read its sections.
fn check_unread(config: &Config, deny: bool) -> Result<(), ConfigError> {
    let unread = config.unread();
    if unread.is_empty() {
        return Ok(());
    }
    if !deny {
        for (path, origin) in &unread {
            tracing::warn!(key = %path, origin = %origin, "configuration key was supplied but no module reads it");
        }
        return Ok(());
    }
    let mut problems = Problems::new();
    for (path, origin) in unread {
        problems.list.push(Problem { path, origin: Some(origin), message: String::from("no module reads this key"), flattened: false, truncated: false });
    }
    Err(ConfigError { header: String::from("configuration holds keys nothing reads"), problems })
}
