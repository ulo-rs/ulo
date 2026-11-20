//! What a server serving gateways needs from the hub (transports DESIGN §3.5): the gateway table
//! it serves, the handshake decision for each upgrade request, and, through
//! [`Switch::serve`](crate::Switch::serve), the connection driver. The hand-off on the HTTP
//! server's port and `ulo-ws-hyper`'s standalone server both serve through it, so a server is the
//! socket code around these three calls.
//!
//! ```ignore
//! # // reason: a fragment of a server's lifecycle: it elides the HTTP head parsing and the upgraded I/O
//! // `prepare`
//! let table = GatewayTable::own_port("my_server::Server", &mounted, &defaults, &mut failures);
//! // `bind`
//! table.start().await?;
//! table.bound_at(&addrs);
//! // each upgrade request
//! match table.handshake(head, peer).await {
//!     Handshake::Switch(switch) => {
//!         write(switch.response());
//!         switch.serve(async move { upgraded_io.await });
//!     }
//!     Handshake::Refuse(refusal) => write(refusal.into_response()),
//! }
//! // `drain`, `close`
//! table.drain(token).await;
//! table.close().await;
//! ```

use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, PoisonError, RwLock};
#[cfg(feature = "conformance-probe")]
use std::sync::atomic::{AtomicUsize, Ordering};

#[cfg(feature = "conformance-probe")]
use futures_core::future::BoxFuture;

use http::StatusCode;
use http::request::Parts;
use ulo::{AppHandle, Bound, BoxError, DrainToken, Mounted, Runtime, Timer, Transport};
use ulo_transport::{Count, ShedLog};
use ulo_transport::prepare::Failures;

use crate::broadcast::InMemory;
use crate::connection::{self, Accept, Handshake, Refusal, Tracker};
use crate::gateway::{GatewayRuntime, Port, build_table, check_defaults, same_path};
use crate::handoff::after_init;
use crate::module::HubMeta;
use crate::origin::Origins;
use crate::rooms::Hub;
use crate::transport::{Ws, WsConnect};

/// How long `close` lets the connections end on their own, once the drain has begun, before it
/// aborts them.
const CLOSE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// The settings a gateway reads where its attribute leaves one unset: `WsModule`'s for the
/// gateways on the HTTP server's port, a standalone server's for those on its own.
#[derive(Clone, Debug, Default)]
pub struct GatewayDefaults {
    pub(crate) max_message: Option<u64>,
    pub(crate) max_connections: Count,
    pub(crate) max_inflight: Count,
    pub(crate) server_max_inflight: Count,
    pub(crate) max_outbound: Count,
    pub(crate) ping_interval: Bound,
    pub(crate) pong_timeout: Bound,
    pub(crate) origins: Origins,
    pub(crate) messages_read: MessagesRead,
    pub(crate) answer_hold: AnswerHold,
}

/// How many data messages a table's connections have read off their sockets, a Text or Binary
/// message counted as it is read, before it is handled or waits for a place. Every clone reads the
/// same count. Under `server_max_inflight` it shows the connections stop reading: once every place
/// is taken it rises by at most one message per connection.
///
/// Exported and counting under the `conformance-probe` feature, which is for the conformance
/// tests, not an application feature; without it the read loop counts nothing. Hidden from the
/// documentation and outside the crate's semver promise.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct MessagesRead(#[cfg(feature = "conformance-probe")] Arc<AtomicUsize>);

impl MessagesRead {
    #[cfg(feature = "conformance-probe")]
    pub fn get(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }

    pub(crate) fn add(&self) {
        #[cfg(feature = "conformance-probe")]
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}

/// A wait between a message's answer and its queueing, for the conformance tests: called with the
/// message's event when its answer, an error or a streamed answer's start included, is about to be
/// pushed to the connection's outbound queue, and awaited before the push. It holds open the point
/// where `max_inflight` counts a message out, which a test of the order of two answers under a
/// bound of one delays.
///
/// Exported and holding under the `conformance-probe` feature, which is for the conformance tests,
/// not an application feature; without it the connection awaits nothing there. Hidden from the
/// documentation and outside the crate's semver promise.
#[doc(hidden)]
#[derive(Clone, Default)]
pub struct AnswerHold(#[cfg(feature = "conformance-probe")] Option<Arc<HoldFn>>);

/// A test's wait, given the message's event.
#[cfg(feature = "conformance-probe")]
type HoldFn = dyn Fn(&str) -> BoxFuture<'static, ()> + Send + Sync;

impl AnswerHold {
    /// `hold(event)` is awaited before each answer is queued.
    #[cfg(feature = "conformance-probe")]
    pub fn new(hold: impl Fn(&str) -> BoxFuture<'static, ()> + Send + Sync + 'static) -> AnswerHold {
        AnswerHold(Some(Arc::new(hold)))
    }

    pub(crate) async fn before_queueing(&self, event: &str) {
        #[cfg(feature = "conformance-probe")]
        if let Some(hold) = &self.0 {
            hold(event).await;
        }
        #[cfg(not(feature = "conformance-probe"))]
        let _ = event;
    }
}

impl fmt::Debug for AnswerHold {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AnswerHold")
    }
}

impl GatewayDefaults {
    /// The hold the connections of the table these defaults build await before queueing each
    /// answer. Test support under the `conformance-probe` feature.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn answer_hold(mut self, hold: AnswerHold) -> Self {
        self.answer_hold = hold;
        self
    }

    /// The count of data messages read by the connections of the table these defaults build,
    /// shared with every clone of them. A server holding them hands it out before it moves into
    /// the app. Test support under the `conformance-probe` feature.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn messages_read(&self) -> MessagesRead {
        self.messages_read.clone()
    }

    /// Bytes per message after reassembly: 64 MiB at the default. Zero is refused in `prepare`.
    pub fn max_message(mut self, bytes: u64) -> Self {
        self.max_message = Some(bytes);
        self
    }

    /// Connections per gateway: unbounded at `Count::Default`. Over it a connection is refused as
    /// `Unavailable`, closed with 1013 after the upgrade, or under `refuse = handshake` answered 503
    /// with `Retry-After` before it. `Count::Max(0)` is refused in `prepare`.
    pub fn max_connections(mut self, connections: Count) -> Self {
        self.max_connections = connections;
        self
    }

    /// Messages in flight per connection, over which the connection stops reading: 64 at
    /// `Count::Default`, none at `Count::Unlimited`. `Count::Max(0)` is refused in `prepare`.
    /// Answers to different messages, a stream's items among them, may interleave, each carrying
    /// its message's `id`; only unary answers (one frame, a `complete` or an error) under a bound
    /// of one come back in the order their messages arrived.
    pub fn max_inflight(mut self, messages: Count) -> Self {
        self.max_inflight = messages;
        self
    }

    /// Messages in flight across every connection of the server, over which every connection
    /// stops reading until a place frees, a message read as the last place went waiting unhandled
    /// for one: 1,024 at `Count::Default` (`Count::DEFAULT_MAX_INFLIGHT`), none at
    /// `Count::Unlimited`. The pressure lands in the clients' TCP buffers rather than as refusals.
    /// `Count::Max(0)` is refused in `prepare`.
    pub fn server_max_inflight(mut self, messages: Count) -> Self {
        self.server_max_inflight = messages;
        self
    }

    /// Queued outbound messages per connection, over which the gateway's `overflow` applies.
    pub fn max_outbound(mut self, messages: Count) -> Self {
        self.max_outbound = messages;
        self
    }

    /// The keep-alive Ping's period: 30 seconds at `Bound::Default`; `Bound::Unbounded` sends none.
    pub fn ping_interval(mut self, interval: Bound) -> Self {
        self.ping_interval = interval;
        self
    }

    /// How long a Pong may take before the connection ends as `Lost`: 30 seconds at
    /// `Bound::Default`.
    pub fn pong_timeout(mut self, timeout: Bound) -> Self {
        self.pong_timeout = timeout;
        self
    }

    /// The pages whose handshakes the gateways admit, by the `Origin` a browser sends:
    /// `.origins(["https://app.example"])` lists them, `.origins(Origins::Any)` admits every one.
    /// Unset, a page on the server's own origin is admitted, and a handshake without `Origin`
    /// under every setting; a refused one answers 403 before the upgrade. [`Origins`] states the
    /// comparison. `prepare` refuses a listed entry that is not an origin.
    pub fn origins(mut self, origins: impl Into<Origins>) -> Self {
        self.origins = origins.into();
        self
    }
}

/// The gateways one server serves, with the application's rooms, its runtime, and the
/// connections the server has open. Cheap to clone; every clone is the same table. Its last clone
/// dropped without [`close`](Self::close), as a server's is when its app drops unclosed, aborts the
/// connections it upgraded.
#[derive(Clone)]
pub struct GatewayTable {
    inner: Arc<Inner>,
}

struct Inner {
    gateways: Vec<Arc<GatewayRuntime>>,
    hub: Arc<Hub>,
    app: AppHandle,
    runtime: Arc<dyn Runtime>,
    tracker: Arc<Tracker>,
    /// Counts the connections refused over a gateway's `max_connections`.
    shed: ShedLog,
    /// On the HTTP server's port, the bound listeners of the HTTP servers sharing the hand-off,
    /// each with its shed log; read on every upgrade, written once per server at its bind.
    listeners: RwLock<Listeners>,
}

/// The table's last clone dropped, its server dropped without `close`, aborts the connections
/// left: a connection's task holds the tracker and not the table, so nothing else ends one before
/// its peer hangs up.
impl Drop for Inner {
    fn drop(&mut self) {
        self.tracker.abort_all();
    }
}

/// The listeners [`GatewayTable::listening`] recorded, as many as the HTTP servers bound, never
/// more: a request is matched to one, and no address a request arrives on adds one.
#[derive(Default)]
struct Listeners(Vec<(SocketAddr, ShedLog)>);

impl Listeners {
    /// Adds each of `addrs` not recorded yet, its log from `log` named by the address.
    fn register(&mut self, addrs: &[SocketAddr], log: impl Fn() -> ShedLog) {
        for addr in addrs {
            if !self.0.iter().any(|(recorded, _)| recorded == addr) {
                let shed = log();
                shed.set_server(addr.to_string());
                self.0.push((*addr, shed));
            }
        }
    }

    fn log_for(&self, local: SocketAddr) -> Option<ShedLog> {
        let listener = matched(self.0.iter().map(|(addr, _)| *addr), local)?;
        self.0.iter().find(|(addr, _)| *addr == listener).map(|(_, shed)| shed.clone())
    }
}

/// The listener among `listeners` a connection whose local address is `local` arrived on: one on
/// `local`'s port whose address is `local`'s, else one on the port bound to the unspecified address
/// of `local`'s family, else one bound to the other family's, as a dual-stack `[::]` listener
/// accepts IPv4. An IPv4-mapped IPv6 address is read as the IPv4 address it maps.
fn matched(listeners: impl Iterator<Item = SocketAddr> + Clone, local: SocketAddr) -> Option<SocketAddr> {
    let ip = local.ip().to_canonical();
    let on_port = listeners.filter(|listener| listener.port() == local.port());
    let exact = on_port.clone().find(|listener| listener.ip().to_canonical() == ip);
    let family = || on_port.clone().find(|listener| listener.ip().is_unspecified() && listener.is_ipv4() == ip.is_ipv4());
    let any = || on_port.clone().find(|listener| listener.ip().is_unspecified());
    exact.or_else(family).or_else(any)
}

impl GatewayTable {
    /// The gateways declared `port = own`, for a standalone server's `prepare`: each connect
    /// handler `ulo-ws` mounted under `WsConnect` paired by controller with its message handlers,
    /// `defaults` filling what a gateway's attribute leaves unset.
    ///
    /// Pushes onto `failures`, naming `server`, every zero default, every gateway the table
    /// refuses (a path not starting with `/` or carrying `{param}`, two gateways on one path,
    /// reserved event names, two handlers for one event, every zero limit), the absence of
    /// any `port = own` gateway, and the broadcast adapter's own refusal from its `prepare`. The
    /// table is built either way; a server reports `failures` and binds nothing when any was
    /// pushed.
    ///
    /// Rooms and broadcasts are the application's `WsModule`'s; an application without one gets
    /// an in-memory adapter of the table's own, which no `Dep<Rooms>` reaches.
    pub fn own_port(server: &str, mounted: &Mounted<'_, Ws>, defaults: &GatewayDefaults, failures: &mut Failures) -> GatewayTable {
        check_defaults(server, defaults, failures);
        let connects = mounted.handlers_of::<WsConnect>();
        let gateways = build_table(&connects, mounted.handlers(), Port::Own, defaults, failures);
        if gateways.is_empty() {
            failures.push(format!(
                "`{server}` serves the gateways declared `port = own`, and none is; a gateway without it is served on the HTTP \
                 server's port"
            ));
        }
        let hub = mounted
            .module_meta::<HubMeta>()
            .into_iter()
            .find_map(|(_, meta)| meta.hub.clone())
            .unwrap_or_else(|| Arc::new(Hub::new(Arc::new(InMemory::new()))));
        if let Err(error) = hub.prepare() {
            failures.push_error(error);
        }
        GatewayTable::new(gateways, hub, mounted.app().clone(), Arc::clone(mounted.runtime()), defaults)
    }

    /// `defaults` gives the table its `server_max_inflight`, the count of messages read and the hold
    /// before each answer is queued.
    pub(crate) fn new(
        gateways: Vec<Arc<GatewayRuntime>>,
        hub: Arc<Hub>,
        app: AppHandle,
        runtime: Arc<dyn Runtime>,
        defaults: &GatewayDefaults,
    ) -> Self {
        hub.record_gateways(&gateways);
        let tracker = Tracker::new(defaults.server_max_inflight.max_inflight(), defaults.messages_read.clone(), defaults.answer_hold.clone());
        let shed = ShedLog::new(<WsConnect as Transport>::KEY, Arc::clone(&runtime) as Arc<dyn Timer>);
        GatewayTable { inner: Arc::new(Inner { gateways, hub, app, runtime, tracker, shed, listeners: RwLock::default() }) }
    }

    /// Each gateway's path, its controller's prefix joined.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.inner.gateways.iter().map(|gateway| &*gateway.path)
    }

    /// Starts the application's broadcast delivery, once whichever server starts it, and each
    /// gateway's `AfterInit`, each a task on the app's runtime. A server calls it once bound, and
    /// it returns once the broadcast adapter's subscription is live. The adapter's failure to
    /// subscribe is the error, which a server returns from `bind`; no `AfterInit` has started then.
    pub async fn start(&self) -> Result<(), BoxError> {
        self.inner.hub.start(&self.inner.runtime).await?;
        after_init(&self.inner.gateways, &*self.inner.runtime);
        Ok(())
    }

    /// The decision on one upgrade request, made from its head alone: the gateway at its path
    /// (404 when none), RFC 6455's checks (405 for a method other than GET, 400 for HTTP/1.0, a
    /// missing `Connection: Upgrade`, `Upgrade: websocket` or `Sec-WebSocket-Key`, 426 for a
    /// version other than 13), 403 for an `Origin` the defaults' [`origins`](GatewayDefaults::origins)
    /// refuse, 503 with `Retry-After` once the drain has begun (on the HTTP
    /// server's port that server's own drain answers first), and the first of the gateway's
    /// subprotocols the client offered. Under `refuse = handshake` the connection phase runs
    /// here, its connect guards and `OnConnect`, and a refusal answers the status and headers HTTP
    /// answers its error with, as [`ConnectRefused`](crate::ConnectRefused) states, a connection
    /// over `max_connections` 503 with `Retry-After`.
    ///
    /// The challenge of a 401 whose error names none and the `Retry-After` of a 503 the server
    /// gives for its own state are the HTTP server's `challenge` and `shed_retry_after` on its
    /// port, and `Bearer` and one second, its defaults, on a standalone server's.
    ///
    /// `peer` is the client's address, which `ConnectionInfo::peer` reports.
    ///
    /// A connection refused over `max_connections` is counted into the table's shed log, whose
    /// `server` field [`bound_at`](Self::bound_at) names.
    pub async fn handshake(&self, head: Parts, peer: Option<SocketAddr>) -> Handshake {
        self.handshake_logged(head, peer, self.inner.shed.clone()).await
    }

    /// [`handshake`](Self::handshake) on the HTTP server's port, where the gateways are reached
    /// through every listener of every HTTP server sharing the hand-off: a refusal is counted into
    /// the shed log of the listener `local` arrived on, as [`matched`] finds it, or
    /// into the table's own, named `http port`, when the host reported no local address or no
    /// listener matches.
    pub(crate) async fn handshake_on(&self, head: Parts, peer: Option<SocketAddr>, local: Option<SocketAddr>) -> Handshake {
        let shed = local.and_then(|local| self.inner.listeners.read().unwrap_or_else(PoisonError::into_inner).log_for(local));
        self.handshake_logged(head, peer, shed.unwrap_or_else(|| self.inner.shed.clone())).await
    }

    /// Records the addresses an HTTP server sharing the hand-off bound, each with a shed log
    /// named by it, the name that server's own shed log carries for a single listener.
    pub(crate) fn listening(&self, addrs: &[SocketAddr]) {
        let runtime = Arc::clone(&self.inner.runtime) as Arc<dyn Timer>;
        self.inner.listeners.write().unwrap_or_else(PoisonError::into_inner).register(addrs, || {
            ShedLog::new(<WsConnect as Transport>::KEY, Arc::clone(&runtime))
        });
    }

    async fn handshake_logged(&self, head: Parts, peer: Option<SocketAddr>, shed: ShedLog) -> Handshake {
        let Some(gateway) = self.gateway_for(head.uri.path()) else {
            return Handshake::Refuse(Refusal::new(StatusCode::NOT_FOUND, "no gateway at this path"));
        };
        connection::handshake(self.accept(gateway, shed), head, peer).await
    }

    /// Names the server in the `server` field of the lines the table's shed log writes, a
    /// standalone server's call once bound: `addrs`, the addresses it bound, comma-separated.
    /// Unnamed, the field is empty.
    pub fn bound_at(&self, addrs: &[SocketAddr]) {
        self.inner.shed.set_server(addrs.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "));
    }

    /// Names the server on the table's own shed log, where it binds no address.
    pub(crate) fn shed_server(&self, server: &str) {
        self.inner.shed.set_server(server);
    }

    /// Raises the drain on this table's connections: idle ones close with 1001 at once, busy ones
    /// stop reading, finish their messages and then close. Returns once every connection has
    /// ended; a handshake from here on is refused 503.
    pub async fn drain(&self, token: DrainToken) {
        self.inner.tracker.drain(token).await;
    }

    /// Aborts every connection left, returns once their tasks have ended, and stops broadcast
    /// delivery. A clock that refuses the grace, its runtime shut down, aborts them at once.
    pub async fn close(&self) {
        let grace = self.inner.runtime.try_sleep(CLOSE_GRACE);
        let grace = Box::pin(async move {
            if let Err(refused) = grace.await {
                tracing::debug!(%refused, "the runtime timing the WebSocket close refused its grace; the connections are aborted without one");
            }
        });
        self.inner.tracker.close(grace).await;
        self.inner.hub.stop();
    }

    pub(crate) fn gateway_for(&self, path: &str) -> Option<Arc<GatewayRuntime>> {
        self.inner.gateways.iter().find(|gateway| same_path(&gateway.path, path)).cloned()
    }

    fn accept(&self, gateway: Arc<GatewayRuntime>, shed: ShedLog) -> Accept {
        Accept {
            gateway,
            hub: Arc::clone(&self.inner.hub),
            app: self.inner.app.clone(),
            runtime: Arc::clone(&self.inner.runtime),
            tracker: Arc::clone(&self.inner.tracker),
            shed,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::SocketAddr;
    use std::sync::Arc;

    use ulo::testing::ScriptedTimer;
    use ulo_transport::ShedLog;

    use super::{Listeners, matched};

    fn addr(text: &str) -> SocketAddr {
        text.parse().expect("a socket address")
    }

    /// The listeners `addrs` bound, and how many logs registering them built.
    fn registered(addrs: &[&str]) -> (Listeners, usize) {
        let clock = ScriptedTimer::new();
        let built = std::cell::Cell::new(0);
        let mut listeners = Listeners::default();
        let addrs: Vec<SocketAddr> = addrs.iter().map(|text| addr(text)).collect();
        listeners.register(&addrs, || {
            built.set(built.get() + 1);
            ShedLog::new("ws_connect", Arc::new(clock.clone()))
        });
        (listeners, built.get())
    }

    #[test]
    fn a_connection_is_matched_to_the_listener_it_arrived_on() {
        let listeners = [addr("0.0.0.0:8080"), addr("[::]:8080"), addr("127.0.0.1:9090"), addr("[::]:7070")];
        let rows: &[(&str, Option<&str>)] = &[
            ("127.0.0.1:8080", Some("0.0.0.0:8080")),
            ("192.0.2.7:8080", Some("0.0.0.0:8080")),
            ("[::ffff:192.0.2.7]:8080", Some("0.0.0.0:8080")),
            ("[::1]:8080", Some("[::]:8080")),
            ("[2001:db8::5]:8080", Some("[::]:8080")),
            ("127.0.0.1:9090", Some("127.0.0.1:9090")),
            ("[::ffff:127.0.0.1]:9090", Some("127.0.0.1:9090")),
            ("10.0.0.1:9090", None),
            ("10.0.0.1:7070", Some("[::]:7070")),
            ("127.0.0.1:6060", None),
        ];
        for (local, expected) in rows {
            assert_eq!(matched(listeners.iter().copied(), addr(local)), expected.map(addr), "a connection to {local}");
        }
    }

    /// One wildcard listener reached through three destination addresses is one log, and a
    /// request matching no listener adds none.
    #[test]
    fn the_logs_are_the_listeners_bound_whatever_addresses_requests_arrive_on() {
        let (mut listeners, built) = registered(&["[::]:8080"]);
        assert_eq!(built, 1);
        for local in ["127.0.0.1:8080", "[::1]:8080", "[2001:db8::5]:8080"] {
            assert!(listeners.log_for(addr(local)).is_some(), "a connection to {local} has the listener's log");
        }
        assert!(listeners.log_for(addr("127.0.0.1:1")).is_none(), "an unmatched address takes the shared fallback");
        assert_eq!(listeners.0.len(), 1, "one log for one listener");
        listeners.register(&[addr("[::]:8080")], || unreachable!("a listener recorded twice"));
        assert_eq!(listeners.0.len(), 1);
    }

    /// A second HTTP server sharing the hand-off, an admin port beside the public one, adds its
    /// listeners beside the first server's.
    #[test]
    fn a_second_server_s_listeners_join_the_first_s() {
        let (mut listeners, _) = registered(&["0.0.0.0:8080"]);
        let clock = ScriptedTimer::new();
        listeners.register(&[addr("127.0.0.1:9000")], || ShedLog::new("ws_connect", Arc::new(clock.clone())));
        assert!(listeners.log_for(addr("192.0.2.7:8080")).is_some(), "the first server's listener is still matched");
        assert!(listeners.log_for(addr("127.0.0.1:9000")).is_some(), "the second server's listener is matched");
        assert_eq!(listeners.0.len(), 2);
    }
}
