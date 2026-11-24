//! Support for the code `ulo-http-macros` generates, and for `ulo-ws`'s refused handshake. Not
//! part of the public API: names and shapes here change with their users.

use std::borrow::Cow;
use std::marker::PhantomData;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use serde::de::DeserializeOwned;

pub use ulo_transport as transport;

use http::{Extensions, HeaderMap, HeaderValue, StatusCode};
use ulo_transport::CallError;

use crate::backend::HttpConfig;
use crate::extract::{Host, Path};
use crate::handler::HttpHandler;
use crate::render;
use crate::upgrade::ServingConfig;

/// The status and the headers HTTP answers `error` with: the status `WithHttpStatus` set when it is
/// 400 to 599, otherwise the kind's, and the headers that status calls for, `WWW-Authenticate`
/// with the error's own challenge on a 401, `Retry-After` from a `RetryAfter` detail on a 429 or a
/// 503. A WebSocket handshake refused under `refuse = handshake` is answered with them, and a 401
/// whose error names no challenge with [`HandshakeAnswers::challenge`].
pub fn error_head(error: &CallError) -> (StatusCode, HeaderMap) {
    let status = render::rendered_status(error);
    let mut headers = HeaderMap::new();
    render::status_headers(error, status, None, &mut headers);
    (status, headers)
}

/// The 500 problem document an unclaimed internal error renders as, `detail` `internal error`:
/// what an embedding answers in place of an app response its host cannot carry as written.
pub fn internal_error() -> crate::Response {
    render::problem(&CallError::new(ulo_transport::ErrorKind::Internal, "internal error"), &HttpConfig::default())
}

/// The 400 problem document a server reading requests off the wire answers an HTTP/1.1 request
/// carrying no `Host` field with (RFC 9112 §3.2, RFC 6455 §4.2.1 for a WebSocket handshake), with
/// `Connection: close`, `Routing::Unrouted` and the request's version, so the connection closes
/// whatever the client asked of it; `None` for a request of another version or carrying `Host`,
/// an empty one included. `ulo-http-hyper` and `ulo-ws-hyper` each answer with it before their
/// service reads the request.
pub fn missing_host(version: http::Version, headers: &HeaderMap) -> Option<crate::Response> {
    if version != http::Version::HTTP_11 || headers.contains_key(http::header::HOST) {
        return None;
    }
    let mut refused = render::problem(&CallError::new(ulo_transport::ErrorKind::BadRequest, "an HTTP/1.1 request carries no `Host`"), &HttpConfig::default());
    refused.headers_mut().insert(http::header::CONNECTION, HeaderValue::from_static("close"));
    *refused.version_mut() = version;
    refused.extensions_mut().insert(crate::Routing::Unrouted);
    Some(refused)
}

/// The 400 problem document the `ulo_http` service answers an HTTP/1.x request with when two
/// readers could read it differently, more than one `Host` line or `Content-Length` beside
/// `Transfer-Encoding` (RFC 9112 §3.2, §6.3), or when it is a WebSocket handshake carrying
/// `Transfer-Encoding`, with `Connection: close`, `Routing::Unrouted` and the request's version;
/// `None` for any other request. The service's own check, so `ulo-ws-hyper` answers a handshake on
/// its own port exactly as the HTTP server's port does.
pub fn ambiguous_head(version: http::Version, headers: &HeaderMap) -> Option<crate::Response> {
    crate::service::ambiguous(version, headers, &HttpConfig::default())
}

/// What the server answering a WebSocket handshake adds to its refusals, as it adds them to its
/// own errors and refusals. Each header is built when a refusal asks for it, so a handshake that
/// is not refused builds none and logs nothing for a challenge that is not a valid header value.
#[derive(Clone, Debug)]
pub struct HandshakeAnswers(Option<Arc<HttpConfig>>);

impl HandshakeAnswers {
    /// The answers of a server that is no `ulo_http` server: a standalone WebSocket server's or a
    /// host's, its `challenge` and `shed_retry_after`, each `HttpConfig::default()`'s, `Bearer`
    /// and one second, where it sets none, and no forwarded host.
    pub fn own(challenge: Option<Cow<'static, str>>, shed_retry_after: Option<Duration>) -> HandshakeAnswers {
        let mut config = HttpConfig::default();
        if let Some(challenge) = challenge {
            config.challenge = challenge;
        }
        if let Some(after) = shed_retry_after {
            config.shed_retry_after = after;
        }
        HandshakeAnswers(Some(Arc::new(config)))
    }

    /// `WWW-Authenticate` on a 401 whose error names no challenge: the server's `challenge`.
    pub fn challenge(&self) -> HeaderValue {
        match &self.0 {
            Some(config) => render::challenge_value(&config.challenge),
            None => render::challenge_value(&HttpConfig::default().challenge),
        }
    }

    /// `Retry-After` on a 503 refusing for the server's own state, the drain or a connection
    /// limit: the server's `shed_retry_after`.
    pub fn retry_after(&self) -> HeaderValue {
        match &self.0 {
            Some(config) => render::retry_after(config.shed_retry_after),
            None => render::retry_after(HttpConfig::default().shed_retry_after),
        }
    }

    /// The host a handshake from `peer` carrying `headers` addressed as the server's trusted
    /// proxy reports it, which the `Origin` check compares in place of `Host`: read only under the
    /// server's `behind_proxy`, from a peer in its ranges, and from the header it names
    /// (`X-Forwarded-Host` with `XForwardedFor`, `host=` in `Forwarded`). `None` otherwise, and
    /// always for a standalone WebSocket server, which has no such setting.
    pub fn forwarded_host(&self, headers: &HeaderMap, peer: Option<SocketAddr>) -> Option<String> {
        let config = self.0.as_ref()?;
        crate::client_ip::forwarded_host(peer, headers, config.behind_proxy.as_ref())
    }
}

/// The answers of the server whose request carries `extensions`: the `ulo_http` server or embedding
/// that handed the request to an `UpgradeHandler`, or, for a request no such server handed over,
/// a standalone WebSocket server's, `HttpConfig::default()`'s `Bearer` and one second.
pub fn handshake_answers(extensions: &Extensions) -> HandshakeAnswers {
    HandshakeAnswers(extensions.get::<ServingConfig>().map(|serving| Arc::clone(&serving.0)))
}

/// The seal of [`HttpCarried`](crate::HttpCarried): implemented for `Http` here and for
/// `ulo_grpc::Grpc` in `ulo-grpc`, and by no other transport.
pub trait Carried {}

/// `(&&PathProbe::<P>::new()).__ulo_path(handler)`: `handler.reads_path::<T>()` for a `Path<T>`
/// parameter, `handler` unchanged for any other, ranked by autoref at the concrete parameter type,
/// so an alias of `Path` is checked as one. Every probe method here is named `__ulo_*`, so it does
/// not meet a user trait's method of a common name at the call (`ulo::__private`).
pub struct PathProbe<P>(PhantomData<fn() -> P>);

impl<P> PathProbe<P> {
    pub fn new() -> Self {
        PathProbe(PhantomData)
    }
}

impl<P> Default for PathProbe<P> {
    fn default() -> Self {
        PathProbe::new()
    }
}

pub trait ViaPath {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler;
}

impl<T: DeserializeOwned + 'static> ViaPath for &PathProbe<Path<T>> {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler {
        handler.reads_path::<T>()
    }
}

pub trait NotPath {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler;
}

impl<P> NotPath for PathProbe<P> {
    fn __ulo_path(&self, handler: HttpHandler) -> HttpHandler {
        handler
    }
}

/// `(&&HostProbe::<P>::new()).__ulo_host(handler)`: `handler.reads_host::<T>()` for a `Host<T>`
/// or `Option<Host<T>>` parameter, `handler` unchanged for any other, ranked by autoref as
/// `PathProbe` is.
pub struct HostProbe<P>(PhantomData<fn() -> P>);

impl<P> HostProbe<P> {
    pub fn new() -> Self {
        HostProbe(PhantomData)
    }
}

impl<P> Default for HostProbe<P> {
    fn default() -> Self {
        HostProbe::new()
    }
}

pub trait ViaHost {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler;
}

impl<T: 'static> ViaHost for &HostProbe<Host<T>> {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler {
        handler.reads_host::<T>()
    }
}

impl<T: 'static> ViaHost for &HostProbe<Option<Host<T>>> {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler {
        handler.reads_host::<T>()
    }
}

pub trait NotHost {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler;
}

impl<P> NotHost for HostProbe<P> {
    fn __ulo_host(&self, handler: HttpHandler) -> HttpHandler {
        handler
    }
}

/// `query`, an absent one read as empty, as `T` by `Query<T>`'s rules, a failure redacted through
/// `app`: a WebSocket handshake's `UpgradeHead::query`.
pub fn query<T: DeserializeOwned>(query: Option<&str>, app: &ulo::AppHandle) -> Result<T, ulo_transport::ExtractError> {
    crate::extract::decode_query(query, app)
}
