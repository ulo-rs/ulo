use serde::Deserialize;
use ulo::Secret;
use ulo_config::{Config, ConfigError, Env, Problems};

/// What the exporter and the sampler are configured with.
///
/// Read from a configuration section with [`from_config`](Self::from_config), or from the standard
/// `OTEL_*` variable names with [`from_env`](Self::from_env) over an `Env` the application passes,
/// which a test fills from a map.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct OtelOptions {
    /// `service.name`; `OTEL_SERVICE_NAME`.
    pub service_name: Option<String>,
    /// The OTLP endpoint; `OTEL_EXPORTER_OTLP_ENDPOINT`.
    pub endpoint: Option<String>,
    /// Request headers as `key=value,key=value`, a secret since they carry credentials;
    /// `OTEL_EXPORTER_OTLP_HEADERS`.
    pub headers: Option<Secret<String>>,
    /// The ratio of root spans sampled, 1.0 by default; `OTEL_TRACES_SAMPLER_ARG`.
    pub sample_ratio: Option<f64>,
    /// How long the flush at shutdown may take.
    pub flush_timeout_ms: Option<u64>,
}

impl OtelOptions {
    /// The section at `path`, with every problem found.
    pub fn from_config(config: &Config, path: &str) -> Result<OtelOptions, ConfigError> {
        let _ = (config, path);
        todo!()
    }

    /// The `OTEL_*` names read through `env.var(..)`; a name that is set and does not parse is a
    /// problem.
    pub fn from_env(env: &Env) -> Result<OtelOptions, Problems> {
        let _ = env;
        todo!()
    }
}
