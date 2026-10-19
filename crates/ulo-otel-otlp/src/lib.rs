//! OTLP export over HTTP with protobuf (integrations DESIGN §6.4), the exporter `ulo-otel` takes.
//!
//! ```ignore
//! let otel = ulo_otel::Otel::new(options, ulo_otel_otlp::OtlpExporter::http())?;
//! ```
//!
//! By default the HTTP client is the crate's own over `ureq`, blocking inside its async `send_bytes`
//! on the batch processor's thread, so an export reaches a receiver from a caller with no runtime
//! and the tree holds no tokio. The `reqwest-blocking` feature selects reqwest's blocking client
//! instead, which runs tokio on a thread of its own and never touches the app's.
//!
//! `ureq`'s rustls client is built with an explicit crypto provider (`ring`, the one the workspace's
//! rustls is built with) and never asks rustls to resolve one, which panics in a process that
//! links two providers and installs neither. Roots come from the platform's verifier. The
//! `reqwest-blocking` client keeps reqwest's own TLS behaviour.
//!
//! The options' `endpoint` is the receiver's base URL, as `OTEL_EXPORTER_OTLP_ENDPOINT` is, and
//! `/v1/traces` is appended to it. With no endpoint the exporter reads the standard `OTEL_*`
//! variables itself and otherwise sends to `http://localhost:4318/v1/traces`. A request that has
//! not finished in ten seconds fails.
//!
//! # The process environment
//!
//! Only variables the OpenTelemetry specification defines are read from the process environment,
//! and they are read by the exporter crate and the SDK beneath this one, not through the `Env`
//! the application passes to [`OtelOptions::from_env`](ulo_otel::OtelOptions::from_env). Where
//! ulo passes a value explicitly, that value wins over the environment's for the endpoint (the
//! per-signal `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` included), the request timeout and the
//! protocol. Headers are the exception: `OTEL_EXPORTER_OTLP_HEADERS` and
//! `OTEL_EXPORTER_OTLP_TRACES_HEADERS` entries are merged over the options' headers, so the
//! environment's value wins for a header name both give. The batch processor's `OTEL_BSP_*` and
//! the `OTEL_SPAN_*_LIMIT` names are read by the SDK; the options carry no value for them.
//!
//! One read is not an OpenTelemetry variable: the default `ureq` client takes its proxy from the
//! environment (`ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`) as ureq's defaults do, and
//! the options carry no proxy to override it.

#[cfg(not(feature = "reqwest-blocking"))]
mod client;

use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use http::{HeaderName, HeaderValue};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use ulo::BoxError;
use ulo_otel::{Exporter, OtelOptions};

const TIMEOUT: Duration = Duration::from_secs(10);

/// OTLP over HTTP with protobuf, the exporter's default protocol. Endpoint and headers come from
/// the [`OtelOptions`] it is built with.
#[derive(Clone, Debug, Default)]
pub struct OtlpExporter {
    _private: (),
}

impl OtlpExporter {
    /// The exporter to hand to `Otel::new` when spans go to an OTLP receiver over HTTP.
    pub fn http() -> Self {
        OtlpExporter { _private: () }
    }
}

impl Exporter for OtlpExporter {
    type Span = SpanExporter;

    fn build(self, options: &OtelOptions) -> Result<SpanExporter, BoxError> {
        let mut builder = SpanExporter::builder()
            .with_http()
            .with_protocol(Protocol::HttpBinary)
            .with_timeout(TIMEOUT);

        if let Some(endpoint) = options.endpoint.as_deref().filter(|e| !e.is_empty()) {
            builder = builder.with_endpoint(signal_endpoint(endpoint));
        }

        if let Some(headers) = &options.headers {
            builder = builder.with_headers(parse_headers(headers.expose())?);
        }

        #[cfg(not(feature = "reqwest-blocking"))]
        {
            builder = builder.with_http_client(client::UreqClient::new(TIMEOUT)?);
        }

        Ok(builder.build()?)
    }
}

/// The traces URL under a base endpoint; a programmatic endpoint is otherwise used by the exporter
/// as the whole URL, where the environment variable's value is a base.
fn signal_endpoint(base: &str) -> String {
    format!("{}/v1/traces", base.trim_end_matches('/'))
}

/// `key=value,key=value`. A refusal names the entry's position and never its text: the values are
/// credentials. Values are left percent-encoded; the exporter decodes them.
fn parse_headers(text: &str) -> Result<HashMap<String, String>, BoxError> {
    let mut headers = HashMap::new();
    for (n, entry) in text.split(',').map(str::trim).enumerate() {
        if entry.is_empty() {
            continue;
        }
        let Some((key, value)) = entry.split_once('=') else {
            return Err(format!("OTLP header entry {} has no `=`", n + 1).into());
        };
        let (key, value) = (key.trim(), value.trim());
        if HeaderName::from_str(key).is_err() {
            return Err(format!("OTLP header entry {} has an invalid name", n + 1).into());
        }
        if HeaderValue::from_str(value).is_err() {
            return Err(format!("OTLP header entry {} has an invalid value", n + 1).into());
        }
        headers.insert(key.to_owned(), value.to_owned());
    }
    Ok(headers)
}
