use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use http::{HeaderName, HeaderValue};
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use ulo::BoxError;

use crate::proxy::ProxyChoice;

/// reqwest's blocking client with the options' headers applied last and the proxy choice
/// honoured, which the exporter's own default client for the feature does neither of: it merges
/// the environment's headers over the options', and builds its client with no proxy setting.
///
/// The send itself is `opentelemetry-http`'s for `reqwest::blocking::Client`, which blocks on the
/// batch processor's thread.
pub(crate) struct ReqwestClient {
    inner: reqwest::blocking::Client,
    /// Applied after whatever the exporter set, so an explicit header replaces one the exporter
    /// read from `OTEL_EXPORTER_OTLP_HEADERS` or `OTEL_EXPORTER_OTLP_TRACES_HEADERS`.
    headers: Vec<(HeaderName, HeaderValue)>,
}

impl ReqwestClient {
    pub(crate) fn new(
        timeout: Duration,
        headers: Vec<(HeaderName, HeaderValue)>,
        proxy: &ProxyChoice,
    ) -> Result<Self, BoxError> {
        let builder = reqwest::blocking::Client::builder().timeout(timeout).tls_backend_preconfigured(tls_config()?);
        // The proxy URL may carry credentials, so a refusal does not quote it.
        let builder = match proxy {
            ProxyChoice::Environment => builder,
            ProxyChoice::Off => builder.no_proxy(),
            ProxyChoice::Explicit(url) => builder.proxy(
                reqwest::Proxy::all(url).map_err(|_| "the OTLP proxy URL is not a valid proxy URL")?,
            ),
        };
        // Built on a thread of its own: a blocking client built on a thread that runs a tokio
        // runtime panics, and `build` may be called from one.
        let inner = std::thread::Builder::new()
            .spawn(move || builder.build())
            .map_err(|error| format!("failed to spawn the thread for the blocking HTTP client: {error}"))?
            .join()
            .map_err(|_| "the thread building the blocking HTTP client panicked")?
            .map_err(|error| format!("failed to build the blocking HTTP client: {error}"))?;
        Ok(ReqwestClient { inner, headers })
    }
}

/// rustls over `ring` with the platform's verifier, the configuration the default client's ureq is
/// given. reqwest's `rustls-no-provider` would otherwise resolve the process's default provider and
/// panic when none is installed.
fn tls_config() -> Result<rustls::ClientConfig, BoxError> {
    use rustls_platform_verifier::BuilderVerifierExt;

    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    Ok(rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(rustls::ALL_VERSIONS)
        .map_err(|e| format!("the ring crypto provider cannot serve a TLS client: {e}"))?
        .with_platform_verifier()
        .map_err(|e| format!("the platform's certificate verifier is unavailable: {e}"))?
        .with_no_client_auth())
}

impl fmt::Debug for ReqwestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestClient").finish_non_exhaustive()
    }
}

#[async_trait]
impl HttpClient for ReqwestClient {
    async fn send_bytes(&self, mut request: Request<Bytes>) -> Result<Response<Bytes>, HttpError> {
        for (name, value) in &self.headers {
            request.headers_mut().insert(name.clone(), value.clone());
        }
        self.inner.send_bytes(request).await
    }
}
