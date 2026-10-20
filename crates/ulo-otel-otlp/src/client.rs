use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response, ResponseBodyTooLarge};
use ulo::BoxError;
use ureq::tls::{RootCerts, TlsConfig, TlsProvider};

/// The largest response body read; `opentelemetry-http`'s built-in clients use the same bound.
const MAX_RESPONSE_BODY: u64 = 4 * 1024 * 1024;

/// The `HttpClient` over `ureq`: it blocks inside `send_bytes` on the batch processor's thread,
/// where blocking is harmless, and enforces the request timeout including while the response body
/// is read.
///
/// Its rustls configuration is built with an explicit provider through
/// `TlsConfig::unversioned_rustls_crypto_provider`, outside ureq's semver guarantee, which is what
/// the exact pin on `ureq` is for. ureq consults the process's default provider only when none is
/// given, so a process that links two providers and installs none still reaches a receiver.
pub(crate) struct UreqClient {
    agent: ureq::Agent,
    /// Applied after whatever the exporter set, so an explicit header replaces one the exporter
    /// read from `OTEL_EXPORTER_OTLP_HEADERS` or `OTEL_EXPORTER_OTLP_TRACES_HEADERS`.
    headers: Vec<(HeaderName, HeaderValue)>,
}

/// Where the agent's proxy comes from. `Debug` never prints the URL, which may carry credentials.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) enum ProxyChoice {
    /// ureq's default: `ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY` and `NO_PROXY`.
    #[default]
    Environment,
    /// This URL, whatever the environment says.
    Explicit(String),
    /// No proxy, whatever the environment says.
    Off,
}

impl fmt::Debug for ProxyChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ProxyChoice::Environment => "Environment",
            ProxyChoice::Explicit(_) => "Explicit(..)",
            ProxyChoice::Off => "Off",
        })
    }
}

impl UreqClient {
    /// Refuses when the explicit provider cannot serve a TLS client.
    ///
    /// ureq builds its rustls configuration on the first https connection, where a failure would
    /// surface as an export error on every batch; building one here moves the refusal to
    /// construction.
    pub(crate) fn new(
        timeout: Duration,
        headers: Vec<(HeaderName, HeaderValue)>,
        proxy: &ProxyChoice,
    ) -> Result<Self, BoxError> {
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());

        rustls::ClientConfig::builder_with_provider(provider.clone())
            .with_protocol_versions(rustls::ALL_VERSIONS)
            .map_err(|e| format!("the ring crypto provider cannot serve a TLS client: {e}"))?;

        let tls = TlsConfig::builder()
            .provider(TlsProvider::Rustls)
            .unversioned_rustls_crypto_provider(provider)
            .root_certs(RootCerts::PlatformVerifier)
            .build();

        // A 4xx or 5xx answer is a response the exporter reads the status and `retry-after` of,
        // not an error.
        let builder = ureq::Agent::config_builder()
            .timeout_global(Some(timeout))
            .http_status_as_error(false)
            .tls_config(tls);

        // The proxy URL may carry credentials, so a refusal does not quote it.
        let builder = match proxy {
            ProxyChoice::Environment => builder,
            ProxyChoice::Off => builder.proxy(None),
            ProxyChoice::Explicit(url) => builder.proxy(Some(
                ureq::Proxy::new(url).map_err(|_| "the OTLP proxy URL is not a valid proxy URL")?,
            )),
        };

        Ok(UreqClient {
            agent: ureq::Agent::from(builder.build()),
            headers,
        })
    }
}

impl fmt::Debug for UreqClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UreqClient").finish_non_exhaustive()
    }
}

#[async_trait]
impl HttpClient for UreqClient {
    async fn send_bytes(&self, request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        let (parts, body) = request.into_parts();

        // Rebuilt on ureq's own `http` re-export so the two crates' `http` versions need not agree.
        let mut outgoing = ureq::http::Request::builder()
            .method(parts.method.as_str())
            .uri(parts.uri.to_string());
        for (name, value) in &parts.headers {
            if self.headers.iter().any(|(explicit, _)| explicit == name) {
                continue;
            }
            outgoing = outgoing.header(name.as_str(), value.as_bytes());
        }
        for (name, value) in &self.headers {
            outgoing = outgoing.header(name.as_str(), value.as_bytes());
        }
        let outgoing = outgoing.body(body.to_vec())?;

        let mut answer = self.agent.run(outgoing)?;

        let mut reply = Response::builder().status(answer.status().as_u16());
        for (name, value) in answer.headers() {
            reply = reply.header(name.as_str(), value.as_bytes());
        }

        let body = match answer
            .body_mut()
            .with_config()
            .limit(MAX_RESPONSE_BODY)
            .read_to_vec()
        {
            Ok(body) => body,
            Err(ureq::Error::BodyExceedsLimit(_)) => {
                return Err(Box::new(ResponseBodyTooLarge::new()));
            }
            Err(e) => return Err(Box::new(e)),
        };

        Ok(reply.body(Bytes::from(body))?)
    }
}
