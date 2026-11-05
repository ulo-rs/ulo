//! The backend SPI, written without macros (transports DESIGN §3.7). The `http` crate's types are
//! the common language; actix and rocket convert at their edge. A backend crate is conversion
//! plus the drain mapping.

use std::borrow::Cow;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use ulo::{Bound, BoxError};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;
use ulo_transport::Count;

use crate::limits::MB;
use crate::service::AppService;

/// One HTTP server implementation. `ulo_http::Server<B>` implements the core's `Server` over it:
/// it prepares the route table, parses endpoints, loads TLS and checks [`limits`](Self::limits)
/// before handing the backend bound listeners and the [`AppService`] it calls per request.
pub trait Backend: Send + Sync + 'static {
    /// The backend's name, as a `Configure` error naming a limit prints it.
    const NAME: &'static str;

    /// What it can do, checked in `prepare`: asking for what it is declared without is a
    /// `StartupError::Configure` naming the capability and the reason. Read from the backend
    /// value, so a declaration can follow its configuration.
    fn limits(&self) -> BackendLimits;

    /// Takes the listeners the server bound and starts accepting on them, without serving yet.
    /// All-or-nothing: an error closes whatever this call opened. `tls` is the loaded TLS
    /// configuration when the server has one, ALPN offering `h2` then `http/1.1`; the backend
    /// builds its acceptor from it with its runtime's TLS crate.
    fn bind(
        &mut self,
        listeners: Vec<BoundListener>,
        tls: Option<Arc<ServerConfig>>,
        svc: AppService,
        cfg: &HttpConfig,
    ) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Serves until drained, calling `svc` for every request. A response body dropped before its
    /// end is how a peer's reset or a closed connection reaches the execution, as
    /// `CancelReason::Disconnected`, so the backend drops it as soon as it observes either. An
    /// unread request body dropped is not a disconnect.
    fn serve(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// Stops accepting: HTTP/2 GOAWAY, idle HTTP/1.1 keep-alive connections closed, busy ones
    /// marked `Connection: close` on their next response.
    ///
    /// It may return only once its connections have ended, so `close` does not cut a response
    /// still being written; the core drops a drain still running at its drain deadline.
    fn drain(&self) -> impl Future<Output = ()> + Send;

    /// Closes every listener and connection left.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;
}

/// What a backend can do, read in `prepare`: asking a backend for what it is declared without is
/// a `StartupError::Configure` naming the capability and the declared reason. Every capability is
/// held from [`NONE`](Self::NONE), the strict reading, and a backend declares one absent only
/// through its `without_` setter, which takes the reason; its conformance tests confirm each
/// entry.
///
/// ```
/// use ulo_http::BackendLimits;
///
/// let limits = BackendLimits::NONE.without_h2c("the backend's HTTP/2 needs ALPN");
/// assert!(!limits.h2c() && limits.why_not_h2c() == Some("the backend's HTTP/2 needs ALPN"));
/// assert!(limits.tls() && limits.why_not_tls().is_none());
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackendLimits {
    upgrades: Absent,
    h2c: Absent,
    inherited_sockets: Absent,
    port_zero: Absent,
    tls: Absent,
}

/// `None` where the backend or host has the capability, and why it has not otherwise.
pub(crate) type Absent = Option<&'static str>;

impl BackendLimits {
    /// A backend that can do everything.
    pub const NONE: BackendLimits = BackendLimits { upgrades: None, h2c: None, inherited_sockets: None, port_zero: None, tls: None };

    /// Hands over a per-request upgraded I/O; a backend without it refuses a gateway on its port.
    pub const fn upgrades(&self) -> bool {
        self.upgrades.is_none()
    }

    pub const fn without_upgrades(self, reason: &'static str) -> Self {
        BackendLimits { upgrades: Some(reason), ..self }
    }

    pub const fn why_not_upgrades(&self) -> Option<&'static str> {
        self.upgrades
    }

    /// HTTP/2 without TLS.
    pub const fn h2c(&self) -> bool {
        self.h2c.is_none()
    }

    pub const fn without_h2c(self, reason: &'static str) -> Self {
        BackendLimits { h2c: Some(reason), ..self }
    }

    pub const fn why_not_h2c(&self) -> Option<&'static str> {
        self.h2c
    }

    /// Adopts an inherited socket.
    pub const fn inherited_sockets(&self) -> bool {
        self.inherited_sockets.is_none()
    }

    pub const fn without_inherited_sockets(self, reason: &'static str) -> Self {
        BackendLimits { inherited_sockets: Some(reason), ..self }
    }

    pub const fn why_not_inherited_sockets(&self) -> Option<&'static str> {
        self.inherited_sockets
    }

    /// Binds port 0 and reports the port chosen.
    pub const fn port_zero(&self) -> bool {
        self.port_zero.is_none()
    }

    pub const fn without_port_zero(self, reason: &'static str) -> Self {
        BackendLimits { port_zero: Some(reason), ..self }
    }

    pub const fn why_not_port_zero(&self) -> Option<&'static str> {
        self.port_zero
    }

    /// Serves TLS from the configuration the server loads.
    pub const fn tls(&self) -> bool {
        self.tls.is_none()
    }

    pub const fn without_tls(self, reason: &'static str) -> Self {
        BackendLimits { tls: Some(reason), ..self }
    }

    pub const fn why_not_tls(&self) -> Option<&'static str> {
        self.tls
    }
}

/// The server's settings a backend reads, from `ulo_http::Server`'s builder.
#[non_exhaustive]
#[derive(Clone, Debug)]
pub struct HttpConfig {
    /// The body limit a route without `#[meta(BodyLimit(..))]` takes: 2 MiB unset.
    pub body_limit: u64,
    /// The server's in-flight bound; over it a request is answered 503 with `Retry-After`.
    /// `Count::Default`, the value unset, is `Count::DEFAULT_MAX_INFLIGHT` requests, 1,024;
    /// `Count::Unlimited` bounds nothing.
    pub max_inflight: Count,
    /// HTTP/2 `SETTINGS_MAX_CONCURRENT_STREAMS` per connection: excess streams are refused with
    /// `RST_STREAM(REFUSED_STREAM)` by the backend. `Count::Default` leaves the backend's own
    /// value.
    pub max_concurrent_streams: Count,
    /// `Retry-After` on a load-shedding refusal and on a request refused during the drain: one
    /// second unset.
    pub shed_retry_after: Duration,
    /// The `WWW-Authenticate` challenge a 401 carries when its error names none: `Bearer` unset.
    pub challenge: Cow<'static, str>,
    /// Accept HTTP/2 without TLS (prior knowledge).
    pub h2c: bool,
    /// How long the error handlers may take with the `Timeout` a route timeout offers them, their
    /// answer's body read to its end included, before the canonical 504 is sent instead: one
    /// second for `Bound::Default`, timed by the app's `Timer`; `Bound::Unbounded` waits for them.
    /// A body ended on the first poll is written whatever its size; one the server waits on is
    /// buffered up to 1 MiB, a fixed cap, and one yielding more answers the 504 too.
    pub timeout_grace: Bound,
    /// How long a request head may take to arrive, from the moment the backend starts reading it,
    /// a clock that also runs while a keep-alive connection waits for its next request: 30
    /// seconds for `Bound::Default`; `Bound::Unbounded` turns the clock off. Read through
    /// [`header_timeout_after`](Self::header_timeout_after).
    ///
    /// HTTP/1.1 only; HTTP/2 connections are bounded by the backend's own HTTP/2 limits.
    pub header_timeout: Bound,
    /// How long a TLS handshake may take: 30 seconds for `Bound::Default`; `Bound::Unbounded`
    /// turns the clock off. Read through [`handshake_timeout_after`](Self::handshake_timeout_after).
    pub handshake_timeout: Bound,
}

/// `HttpConfig::timeout_grace`'s `Bound::Default`.
const DEFAULT_TIMEOUT_GRACE: Duration = Duration::from_secs(1);

/// `HttpConfig::header_timeout`'s and `HttpConfig::handshake_timeout`'s `Bound::Default`.
const DEFAULT_CONNECTION_TIMEOUT: Duration = Duration::from_secs(30);

impl HttpConfig {
    /// `header_timeout` as a duration, 30 seconds for `Bound::Default`; `None` for
    /// `Bound::Unbounded`, which a backend applies as no clock at all.
    pub fn header_timeout_after(&self) -> Option<Duration> {
        connection_timeout(self.header_timeout)
    }

    /// `handshake_timeout` as a duration, 30 seconds for `Bound::Default`; `None` for
    /// `Bound::Unbounded`, which a backend applies as no clock at all.
    pub fn handshake_timeout_after(&self) -> Option<Duration> {
        connection_timeout(self.handshake_timeout)
    }

    /// `max_inflight` as `Admission` takes it; `None` admits every request.
    pub(crate) fn inflight_limit(&self) -> Option<usize> {
        self.max_inflight.max_inflight()
    }

    /// `timeout_grace` as a duration; `None` for `Bound::Unbounded`.
    pub(crate) fn grace(&self) -> Option<Duration> {
        match self.timeout_grace {
            Bound::Default => Some(DEFAULT_TIMEOUT_GRACE),
            Bound::After(grace) => Some(grace),
            Bound::Unbounded => None,
        }
    }
}

fn connection_timeout(bound: Bound) -> Option<Duration> {
    match bound {
        Bound::Default => Some(DEFAULT_CONNECTION_TIMEOUT),
        Bound::After(after) => Some(after),
        Bound::Unbounded => None,
    }
}

impl Default for HttpConfig {
    fn default() -> Self {
        HttpConfig {
            body_limit: 2 * MB,
            max_inflight: Count::Default,
            max_concurrent_streams: Count::Default,
            shed_retry_after: Duration::from_secs(1),
            challenge: Cow::Borrowed("Bearer"),
            h2c: false,
            timeout_grace: Bound::Default,
            header_timeout: Bound::Default,
            handshake_timeout: Bound::Default,
        }
    }
}
