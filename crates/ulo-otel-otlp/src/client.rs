use std::fmt;
use std::time::Duration;

use async_trait::async_trait;
use opentelemetry_http::{Bytes, HttpClient, HttpError, Request, Response};
use ulo::BoxError;

/// The `HttpClient` over `ureq`, about 20 lines: it blocks inside `send_bytes` on the batch
/// processor's thread, where blocking is harmless, and enforces the request timeout including while
/// the response body is read.
///
/// Its rustls configuration is built with an explicit provider through
/// `TlsConfig::unversioned_rustls_crypto_provider`, outside ureq's semver guarantee, which is what
/// the exact pin on `ureq` is for.
pub(crate) struct UreqClient {
    agent: ureq::Agent,
}

impl UreqClient {
    /// Refuses when the rustls client cannot be built with an explicit provider.
    #[allow(dead_code)]
    pub(crate) fn new(timeout: Duration) -> Result<Self, BoxError> {
        let _ = timeout;
        todo!()
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
        let _ = (&self.agent, request);
        todo!()
    }
}
