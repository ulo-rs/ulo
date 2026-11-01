use std::time::Duration;

use serde::Deserialize;
use ulo::Secret;
use ulo_config::{Config, ConfigError, Env, Problems, Section};

const SERVICE_NAME: &str = "OTEL_SERVICE_NAME";
const ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";
const HEADERS: &str = "OTEL_EXPORTER_OTLP_HEADERS";
const SAMPLER: &str = "OTEL_TRACES_SAMPLER";
const SAMPLER_ARG: &str = "OTEL_TRACES_SAMPLER_ARG";

/// The batch processor's own shutdown timeout, which the flush keeps when the options name none.
const DEFAULT_FLUSH: Duration = Duration::from_secs(5);

/// What the exporter and the sampler are configured with.
///
/// Read from a configuration section with [`from_config`](Self::from_config), or from the standard
/// `OTEL_*` variable names with [`from_env`](Self::from_env) over an `Env` the application passes,
/// which a test fills from a map.
#[derive(Clone, Debug, Default, PartialEq, Deserialize)]
pub struct OtelOptions {
    /// `service.name`; `OTEL_SERVICE_NAME`. Unset, the resource names `unknown_service` with the
    /// executable's name, as the semantic conventions specify.
    pub service_name: Option<String>,
    /// The OTLP receiver's base URL, as `OTEL_EXPORTER_OTLP_ENDPOINT` gives it; the exporter
    /// appends the signal's path.
    pub endpoint: Option<String>,
    /// Request headers as `key=value,key=value`, a secret since they carry credentials;
    /// `OTEL_EXPORTER_OTLP_HEADERS`. The whole text and each value are registered for redaction.
    pub headers: Option<Secret<String>>,
    /// The ratio of root spans sampled, between 0 and 1, 1.0 by default; a span with a parent
    /// follows the parent's decision whatever the ratio. `OTEL_TRACES_SAMPLER_ARG` under
    /// `OTEL_TRACES_SAMPLER=parentbased_traceidratio`.
    pub sample_ratio: Option<f64>,
    /// How long the flush at shutdown may take, 5000 by default and never zero. The shutdown
    /// hook's bound is a second longer. No standard variable names it.
    pub flush_timeout_ms: Option<u64>,
}

impl OtelOptions {
    /// The section at `path`, with every problem found.
    ///
    /// A ratio outside 0 to 1 or a zero flush timeout reads without a problem here and is refused
    /// by `Otel::new`.
    pub fn from_config(config: &Config, path: &str) -> Result<OtelOptions, ConfigError> {
        Section::<OtelOptions>::read(config, path).value().cloned().map_err(ConfigError::clone)
    }

    /// The `OTEL_*` names read through `env.var(..)`; a name that is set and does not parse is a
    /// problem.
    ///
    /// Read: `OTEL_SERVICE_NAME`, `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`,
    /// `OTEL_TRACES_SAMPLER` and `OTEL_TRACES_SAMPLER_ARG`. A variable set to the empty string is
    /// unset, and the sampler's name is matched without regard to case, as the specification of
    /// the variables has it. Sampling is always parent-based, so `OTEL_TRACES_SAMPLER` accepts
    /// `parentbased_always_on`, `parentbased_always_off` and `parentbased_traceidratio`, and any
    /// other sampler is a problem rather than a silent change of meaning;
    /// `OTEL_TRACES_SAMPLER_ARG` is read only under `parentbased_traceidratio`, 1.0 when unset.
    /// The per-signal names, `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` among them, are not read: that
    /// one is a full URL where `endpoint` is a base.
    pub fn from_env(env: &Env) -> Result<OtelOptions, Problems> {
        let read = |name: &str| env.var(name).map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        let mut problems = Problems::new();

        let headers = match read(HEADERS) {
            Some(text) if headers_valid(&text) => Some(Secret::new(text)),
            Some(_) => {
                // The text is a secret: the problem names the shape expected, never what was found.
                problems.at(HEADERS, "expected `key=value` pairs separated by commas");
                None
            }
            None => None,
        };

        let sample_ratio = match read(SAMPLER).map(|name| name.to_ascii_lowercase()).as_deref() {
            None | Some("parentbased_always_on") => None,
            Some("parentbased_always_off") => Some(0.0),
            Some("parentbased_traceidratio") => match read(SAMPLER_ARG) {
                None => None,
                Some(arg) => match arg.parse::<f64>() {
                    Ok(ratio) if ratio_valid(ratio) => Some(ratio),
                    _ => {
                        problems.at(SAMPLER_ARG, format!("expected a ratio between 0 and 1, found `{arg}`"));
                        None
                    }
                },
            },
            Some(other) => {
                problems.at(
                    SAMPLER,
                    format!(
                        "`{other}` is not a sampler ulo-otel offers: it samples parent-based, so one of \
                         `parentbased_always_on`, `parentbased_always_off` or `parentbased_traceidratio`"
                    ),
                );
                None
            }
        };

        problems.into_result()?;
        Ok(OtelOptions {
            service_name: read(SERVICE_NAME),
            endpoint: read(ENDPOINT),
            headers,
            sample_ratio,
            flush_timeout_ms: None,
        })
    }

    /// The refusals `Otel::new` makes, by field name: a ratio that is not a number from 0 to 1, a
    /// zero flush timeout, and headers off the `key=value` grammar.
    pub(crate) fn check(&self) -> Result<(), Problems> {
        let mut problems = Problems::new();
        if let Some(ratio) = self.sample_ratio
            && !ratio_valid(ratio)
        {
            problems.at("sample_ratio", format!("expected a ratio between 0 and 1, found {ratio}"));
        }
        if self.flush_timeout_ms == Some(0) {
            problems.at("flush_timeout_ms", "a zero flush timeout would drop every span still queued at shutdown");
        }
        if let Some(headers) = &self.headers
            && !headers_valid(headers.expose())
        {
            problems.at("headers", "expected `key=value` pairs separated by commas");
        }
        problems.into_result()
    }

    pub(crate) fn flush_timeout(&self) -> Duration {
        self.flush_timeout_ms.map_or(DEFAULT_FLUSH, Duration::from_millis)
    }

    /// Each header's value, which an exporter's error may print without the rest of the text.
    pub(crate) fn header_values(&self) -> impl Iterator<Item = &str> {
        self.headers
            .iter()
            .flat_map(|headers| headers.expose().split(','))
            .filter_map(|pair| pair.split_once('='))
            .map(|(_, value)| value.trim())
            .filter(|value| !value.is_empty())
    }
}

fn ratio_valid(ratio: f64) -> bool {
    (0.0..=1.0).contains(&ratio)
}

/// The OTLP exporter's header grammar: comma-separated members, each a non-empty key, `=`, and a
/// value; an empty member is skipped, as the exporter skips it.
fn headers_valid(text: &str) -> bool {
    text.split(',')
        .map(str::trim)
        .filter(|member| !member.is_empty())
        .all(|member| member.split_once('=').is_some_and(|(key, _)| !key.trim().is_empty()))
}
