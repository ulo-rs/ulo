use std::collections::HashMap;
use std::str::FromStr;
use std::time::Duration;

use http::{HeaderName, HeaderValue};
use opentelemetry_otlp::{Protocol, SpanExporter, WithExportConfig, WithHttpConfig};
use ulo::BoxError;
use ulo_otel::{Exporter, OtelOptions};

use crate::proxy::ProxyChoice;

const TIMEOUT: Duration = Duration::from_secs(10);

/// The HTTP client an [`OtlpExporter`] sends with. A variant exists when its feature is enabled.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    /// The crate's client over `ureq`; no tokio.
    #[cfg(feature = "ureq")]
    Ureq,
    /// reqwest's blocking client, on a tokio runtime of its own.
    #[cfg(feature = "reqwest-blocking")]
    ReqwestBlocking,
}

impl Default for Client {
    /// `ureq` when its feature is enabled, otherwise reqwest's.
    fn default() -> Self {
        #[cfg(feature = "ureq")]
        return Client::Ureq;
        #[cfg(not(feature = "ureq"))]
        return Client::ReqwestBlocking;
    }
}

/// OTLP over HTTP with protobuf, the exporter's default protocol. Endpoint and headers come from
/// the [`OtelOptions`] it is built with.
#[derive(Clone, Debug, Default)]
pub struct OtlpExporter {
    proxy: ProxyChoice,
    client: Client,
}

impl OtlpExporter {
    /// The exporter to hand to `Otel::new` when spans go to an OTLP receiver over HTTP, on
    /// `ureq` when that feature is enabled and on reqwest's blocking client otherwise.
    pub fn http() -> Self {
        OtlpExporter::default()
    }

    /// The same exporter on the named client, for an application with both features enabled.
    pub fn http_over(client: Client) -> Self {
        OtlpExporter { client, ..OtlpExporter::default() }
    }

    /// The client this exporter sends on.
    pub fn client(&self) -> Client {
        self.client
    }

    /// Send through this proxy, whatever `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`
    /// say. A URL that is not a proxy URL is refused by `build`.
    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy = ProxyChoice::Explicit(url.into());
        self
    }

    /// Send directly, whatever the proxy variables say.
    pub fn no_proxy(mut self) -> Self {
        self.proxy = ProxyChoice::Off;
        self
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

        let mut explicit = Vec::new();
        if let Some(headers) = &options.headers {
            let headers = parse_headers(headers.expose())?;
            explicit = decoded(&headers)?;
            builder = builder.with_headers(headers);
        }

        builder = match self.client {
            #[cfg(feature = "ureq")]
            Client::Ureq => builder.with_http_client(crate::client::UreqClient::new(TIMEOUT, explicit, &self.proxy)?),
            #[cfg(feature = "reqwest-blocking")]
            Client::ReqwestBlocking => {
                builder.with_http_client(crate::reqwest_client::ReqwestClient::new(TIMEOUT, explicit, &self.proxy)?)
            }
        };

        Ok(builder.build()?)
    }
}

/// The traces URL under a base endpoint; a programmatic endpoint is otherwise used by the exporter
/// as the whole URL, where the environment variable's value is a base.
fn signal_endpoint(base: &str) -> String {
    format!("{}/v1/traces", base.trim_end_matches('/'))
}

/// The headers as the exporter puts them on the wire: values percent-decoded when they are valid
/// UTF-8 after decoding, as the exporter does.
fn decoded(headers: &HashMap<String, String>) -> Result<Vec<(HeaderName, HeaderValue)>, BoxError> {
    headers
        .iter()
        .map(|(key, value)| {
            let value = percent_encoding::percent_decode_str(value)
                .decode_utf8()
                .map(|v| v.into_owned())
                .unwrap_or_else(|_| value.clone());
            let name = HeaderName::from_str(key).map_err(|_| "an OTLP header name is invalid")?;
            let value =
                HeaderValue::from_str(&value).map_err(|_| "an OTLP header value is invalid")?;
            Ok((name, value))
        })
        .collect()
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
