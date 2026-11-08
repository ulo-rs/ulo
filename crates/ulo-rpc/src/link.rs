//! The link SPI, written without macros (transports DESIGN §5.3): what each RPC transport crate
//! implements once, TCP, UDP, NATS, Redis, RabbitMQ, MQTT and Kafka.
//!
//! A link moves frames. `ulo_rpc::Server<L>` drives its server side, [`Link::listen`], and
//! [`RpcClient`](crate::RpcClient) its client side, [`Link::connect`]; routing, execution, the
//! four shapes, deadlines and cancellation are `ulo-rpc`'s. What differs per link is declared in
//! [`Capabilities`] and asserted by `ulo-rpc-conformance`.
//!
//! The futures and streams a link hands out are polled on whatever executor drives the server or
//! the client, which need not be the runtime the link's I/O belongs to: a link tied to one runs
//! its I/O and its tasks on that runtime itself and hands the results back. The conformance suite
//! calls a client from a plain thread that no runtime runs.
//!
//! Each side is handed the runtime it runs under: a server's link reads its app's from
//! [`Link::prepare`] (`AppHandle::runtime`), and a client's link receives its client's in
//! [`Link::connect`], the app's inside an app and the one given to
//! [`RpcClient::new`](crate::RpcClient::new) outside one. A link whose sockets belong to no runtime
//! spawns its tasks there; a link carrying a runtime of its own, as the tokio links do, ignores it.
//!
//! Beside the SPI, this module holds what the authors of transport links share with the
//! first-party links, so a link outside this repository keeps the same timing and redaction:
//! [`CANCEL_HOLD`] and [`HoldUntil`], how long a link that holds a streamed request's `cancel`
//! until the server's `opened` keeps it; [`panic_redacted`], the redacted error a panic caught
//! outside a task answers; and [`TaskPanics`], through which a panic in a link's own long-lived
//! task ends its server or fails its client's calls. Behind the `conformance-probe` feature it also holds `ClientProbe`, the
//! conformance suite's view of a held `cancel`, which is test support, hidden from the
//! documentation and outside the semver promise. Semver applies to the others as to the rest of
//! the crate's public API: from 0.1.0, a change that breaks a link is released under a new breaking
//! version (`0.x` to `0.(x+1)` before 1.0). A pre-release of 0.1.0 may break it, and Cargo takes
//! a later pre-release of the same version for a requirement on an earlier one, so a link
//! depending on a pre-release pins it with `=`.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::Duration;

use futures_core::stream::BoxStream;
use ulo::{AppHandle, BoundAddr, BoxError, BoxFuture, Runtime, Shape};
use ulo_transport::Count;

use crate::codec::Codec;
use crate::frame::Frame;

#[doc(hidden)]
#[cfg(feature = "conformance-probe")]
pub use crate::held::ClientProbe;
pub use crate::held::{CANCEL_HOLD, HoldUntil};

/// One RPC transport's frame carriage.
pub trait Link: Send + Sync + 'static {
    /// The link's name, `LinkInfo::name` and a `Configure` error's: `tcp`, `nats`, ...
    const NAME: &'static str;

    fn capabilities(&self) -> Capabilities;

    /// What can fail before any I/O: endpoint text, `Tls`, a broker URL. Called from
    /// `Server::prepare`, with the app, whose root module's full type path
    /// (`format!("{:#}", app.root().name())`) is the default competing-consumer group on NATS and
    /// MQTT. It reports [`usable`](Self::usable)'s refusal beside its own, where that refusal
    /// applies to a server. A client's link reports that refusal where the client is built, and the
    /// rest from its first `connect`. A server link keeps `app.timer()` from here for `listen`,
    /// which refuses with [`NoTimer`] when the app had none.
    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        let _ = app;
        async { Ok(()) }
    }

    /// What the link refuses before any I/O and without an app: on a tokio-based link, being built
    /// outside a runtime and given none; on the TCP link, a client endpoint that is not an IP
    /// address and port. [`RpcClient::new`](crate::RpcClient::new) calls it and
    /// refuses the client, and `RpcClientModule` calls it from its init hook, failing the app's
    /// `connect`, so a client's link that can never connect fails where it is set up rather than as
    /// each call's `Unavailable`. It runs when a client takes the link, not when the link is built,
    /// so `Tcp::new(addr).with_handle(h)` written off a runtime passes.
    fn usable(&self) -> Result<(), BoxError> {
        Ok(())
    }

    /// The server's in-flight bound, `Server::max_inflight` as set, called from `Server::prepare`
    /// beside [`prepare`](Self::prepare); `Count::max_inflight` reads it, 1,024 at `Default`. A
    /// link declaring `native_backpressure` stops taking requests from its broker at that bound,
    /// so they wait in the broker where the server would refuse them: AMQP's prefetch, Kafka
    /// pausing its partitions. The server frees a call's place before it settles the call's
    /// [`Ack`], so a link that counts a request in flight until its `Ack` settles, or is dropped,
    /// never hands over one past the bound. Every other link ignores it.
    fn max_inflight(&mut self, calls: Count) {
        let _ = calls;
    }

    /// Server side: subscribe to the mounted handlers' patterns, each with the [`Instances`] a
    /// message on it reaches: [`Instances::One`] for every request and for an event left at its
    /// default, [`Instances::Every`] for an event whose handler is declared `every_instance`. The
    /// listening side alone decides it: a client publishes a request or an event the same way
    /// whichever a server chose. Called from `Server::bind`, all-or-nothing: a failure leaves
    /// nothing subscribed. A link that times its close or its first connect on the app's clock
    /// (TCP, UDP, NATS, MQTT, RabbitMQ, Kafka) refuses with [`NoTimer`] before any I/O when
    /// `prepare` found the app without one.
    fn listen(&self, subscriptions: &[Subscription]) -> impl Future<Output = Result<Inbound, BoxError>> + Send;

    /// Client side: connect. `RpcClient` calls it lazily, on its first call and again after the
    /// connection is lost, passing `runtime`, the runtime the client was built on: its app's inside
    /// an app, the one given to [`RpcClient::new`](crate::RpcClient::new) outside one. A link that
    /// needs a runtime for its client side's tasks spawns them there unless it was given one of its
    /// own; a link that carries its own runtime ignores it.
    fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send;

    /// The link's own close signal, from `Server::drain`: TCP sends `goaway` on every connection,
    /// NATS drains, AMQP cancels its consumers, MQTT unsubscribes, Kafka pauses and commits. The
    /// inbound stream ends once nothing more will arrive.
    fn drain(&self) -> impl Future<Output = ()> + Send;

    /// Closes the link's connections, under the core's `close` bound.
    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send;

    /// The addresses bound, for `App<Bound>::addresses()`, so port 0 reports the port chosen; a
    /// broker link answers none.
    fn bound(&self) -> Vec<BoundAddr> {
        Vec::new()
    }
}

/// What a link's server side delivers.
///
/// The frames' `id`s are the server's correlation: unique among the calls in flight on one
/// inbound stream, so an `in`, `in_end` or `cancel` reaches the call its `req` or `open` started.
/// A link whose callers allocate ids per connection, TCP and UDP, maps each to an id of its own
/// and back on the reply path; a broker link allocates one per native correlation.
pub type Inbound = BoxStream<'static, Delivery>;

/// One frame arriving at the server, with where its replies go and how it is acknowledged.
///
/// A link that loses the caller of calls in flight, a TCP connection closing, delivers `cancel`
/// for each with `reply: None`, which the server reads as `CancelReason::Disconnected`; a
/// `cancel` the caller sent carries its reply path and is `CancelReason::ClientCancelled`.
pub struct Delivery {
    pub frame: Frame,
    /// `None` for an event. A link may attach one to an event to carry the caller's address
    /// ([`ReplyPath::peer`]), which the server reads for `LinkInfo::peer` and never replies on.
    pub reply: Option<ReplyPath>,
    pub ack: Ack,
}

/// Where the replies to one delivery go: the connection on TCP, the sender's address on UDP, the
/// reply subject, queue, topic or channel on a broker. Cheap to clone, every clone the same path;
/// a stream's items and its end go through it one by one.
#[derive(Clone)]
pub struct ReplyPath {
    send: Arc<dyn Fn(Frame) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    peer: Option<SocketAddr>,
}

impl ReplyPath {
    pub fn new<F>(send: F) -> Self
    where
        F: Fn(Frame) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync + 'static,
    {
        ReplyPath { send: Arc::new(send), peer: None }
    }

    /// The caller's address, which the server seeds as `LinkInfo::peer`: TCP and UDP set it.
    pub fn peer(self, peer: SocketAddr) -> Self {
        ReplyPath { peer: Some(peer), ..self }
    }

    /// Sends one reply frame. A send that fails with [`FrameTooLarge`] or [`FrameUnencodable`] is
    /// answered by the server with an `err` frame of kind `internal` under the same `id`, the
    /// second logged at `error`; any other failure means the caller is gone, and the call is
    /// cancelled `Disconnected`. A link encoding with [`Codec::encode_frame`] and passing its
    /// error on with `?` reports an encoding failure as `FrameUnencodable`.
    pub fn send(&self, frame: Frame) -> BoxFuture<'static, Result<(), BoxError>> {
        (self.send)(frame)
    }

    pub(crate) fn peer_addr(&self) -> Option<SocketAddr> {
        self.peer
    }
}

impl fmt::Debug for ReplyPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ReplyPath")
    }
}

/// How one delivery is settled: acknowledged, or rejected without requeue (an unhandled event, so
/// it cannot loop on redelivery). A request or event is settled once its handler completes, and an
/// `in`, `in_end` or `cancel` once the server has handed it to the call it names, a `cancel` after
/// that call is cancelled. AMQP and Kafka settle with their broker; a link can also keep a call its
/// caller cancelled known until the `cancel` settles, its reason written by then. Dropped
/// unsettled it sends nothing, and a broker redelivers the message once the consumer's channel
/// closes.
pub struct Ack {
    settle: Mutex<Option<Box<dyn FnOnce(bool) + Send>>>,
}

impl Ack {
    /// An acknowledgment that `settle` performs, `true` for `ack` and `false` for `reject`.
    pub fn new(settle: impl FnOnce(bool) + Send + 'static) -> Self {
        Ack { settle: Mutex::new(Some(Box::new(settle))) }
    }

    /// An acknowledgment with nothing to do, for a link whose broker acknowledges nothing.
    pub fn none() -> Self {
        Ack { settle: Mutex::new(None) }
    }

    pub fn ack(self) {
        self.settle(true);
    }

    /// AMQP `basic.reject` with `requeue = false`, which routes to a dead-letter exchange when one
    /// is configured; Kafka commits the offset.
    pub fn reject(self) {
        self.settle(false);
    }

    fn settle(&self, accepted: bool) {
        let settle = self.settle.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(settle) = settle {
            settle(accepted);
        }
    }
}

impl fmt::Debug for Ack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Ack")
    }
}

/// A link's client side, from [`Link::connect`]: `send` publishes a frame on `pattern`, with the
/// reply's correlation when one is expected, and `replies` yields every reply-lane frame for this
/// client. A link with a miss signal fails `send` with [`NoDestination`]; one refusing a frame over
/// its size with [`FrameTooLarge`].
///
/// On TCP, UDP, Redis, RabbitMQ and Kafka, frames go out in the order their sends were called: each
/// frame takes its place in the writer's queue when `send` is called, before the returned future
/// is polled, so a streamed request's `in` items arrive in order with `in_end` last and a `cancel`
/// follows the request it names. NATS and MQTT hand each frame to their client library's one
/// command channel as the send is polled. No link writes through a task per frame. A streamed
/// request's `in`, `in_end` and `cancel` on a broker are held until the server acknowledges the
/// `open` and then sent in their own order, a `cancel` alone. The same holds for a server's
/// [`ReplyPath`]s.
pub struct Outbound {
    pub send: Box<dyn Fn(Pattern, Frame, Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    pub replies: BoxStream<'static, Frame>,
}

/// A request's reply correlation, which the link maps onto its own mechanism: the frame's `id` on
/// TCP and UDP, an `_INBOX` subject, `correlation_id`, Correlation Data, a reply header.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReplyTo {
    pub id: u64,
}

/// A pattern, as a handler declares it and a call names it: the subject, channel, queue or topic
/// on a broker.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Pattern(Arc<str>);

impl Pattern {
    pub fn new(pattern: impl Into<Arc<str>>) -> Self {
        Pattern(pattern.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Pattern {
    fn from(pattern: &str) -> Self {
        Pattern(Arc::from(pattern))
    }
}

impl From<String> for Pattern {
    fn from(pattern: String) -> Self {
        Pattern(Arc::from(pattern))
    }
}

/// One pattern a server listens on, and how many of the server instances listening on it a
/// message on it reaches, which [`Link::listen`] receives for each mounted handler.
///
/// ```
/// use ulo_rpc::{Instances, Pattern, Subscription};
///
/// let request = Subscription::from(Pattern::from("invoices.create"));
/// assert_eq!(request.instances, Instances::One);
/// let invalidation = Subscription::new("cache.invalidated", Instances::Every);
/// assert_eq!(invalidation.pattern.as_str(), "cache.invalidated");
/// ```
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Subscription {
    pub pattern: Pattern,
    pub instances: Instances,
}

impl Subscription {
    pub fn new(pattern: impl Into<Pattern>, instances: Instances) -> Self {
        Subscription { pattern: pattern.into(), instances }
    }
}

/// A pattern reaching one instance, as every request's does.
impl From<Pattern> for Subscription {
    fn from(pattern: Pattern) -> Self {
        Subscription { pattern, instances: Instances::One }
    }
}

/// How many of the server instances listening on one pattern receive each message on it.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Instances {
    /// One instance takes each message: every request, and an event left at its default. A NATS
    /// queue group, an MQTT shared subscription `$share/<group>/<pattern>`, one AMQP queue the
    /// instances consume, one Kafka consumer group. A link declared without
    /// [`single_delivery`](Capabilities::single_delivery), Redis Pub/Sub, delivers it to every
    /// instance.
    #[default]
    One,
    /// Every instance receives each message: an event whose handler is declared `every_instance`,
    /// such as a cache invalidation or a configuration reload. A plain NATS subscription, a plain
    /// MQTT topic filter, an AMQP queue of the instance's own bound to the pattern's fanout
    /// exchange, a Kafka consumer of the instance's own outside any shared group, Redis Pub/Sub.
    /// A link declared without [`every_instance`](Capabilities::every_instance), TCP and UDP,
    /// refuses such a handler in `Server::prepare`.
    Every,
}

/// What a link can do and how it is configured, read by `Server::prepare` and by the client, and
/// asserted by the conformance suite in both directions. Built from [`Capabilities::new`] and its
/// setters, since the struct is `#[non_exhaustive]`.
///
/// The public fields report the link's configuration: `binary`, `max_frame`, `ordering`, `shapes`
/// and `delivery` say what the link is set up to speak and how it delivers, and change how the
/// server and client behave on it. `delivery` is given to `new`, and each of the others has a
/// setter of its own name taking the value; `binary` is `false` from `new`, a JSON link, and
/// [`codec`](Self::codec) sets it from the link's codec.
///
/// Every ability named by a `bool` accessor is `true` from `new`, the strict reading, so a link
/// that declares nothing is held to each and a scenario fails where the link cannot keep one. A
/// link declares one absent only through its `without_` setter, which takes the reason, and the
/// `why_not_` accessor answers that reason: `Server::prepare` prints it in a refusal or a `warn`,
/// and the suite in a failure or in the line naming a scenario it skips.
///
/// The abilities are private fields, so no link declares an absence without its reason:
///
/// ```compile_fail,E0616
/// use ulo_rpc::{Capabilities, DeliveryMode};
///
/// let mut capabilities = Capabilities::new(DeliveryMode::FanOut);
/// capabilities.single_delivery = None;
/// ```
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The link's codec carries raw bytes: the server and client speak CBOR on the link, and JSON
    /// where it is `false`. A handler with a binary payload on a JSON link is refused in
    /// `prepare`, and a client call with one fails before any I/O with `binary_unsupported`.
    pub binary: bool,
    /// The largest frame the link carries, `None` for no limit of its own.
    pub max_frame: Option<u64>,
    pub ordering: Ordering,
    /// The call shapes the link carries; a handler of another shape is refused in `prepare`.
    pub shapes: &'static [Shape],
    pub delivery: DeliveryMode,
    native_backpressure: Absent,
    miss_signal: Absent,
    unserved_hold: Absent,
    durable_replies: Absent,
    confirmed_drain: Absent,
    ordered_control: Absent,
    single_delivery: Absent,
    every_instance: Absent,
}

/// `None` where the link has the capability, and why it has not otherwise.
type Absent = Option<&'static str>;

/// Every call shape, for a link that carries them all.
pub const ALL_SHAPES: &[Shape] = &[Shape::Unary, Shape::ServerStreaming, Shape::ClientStreaming, Shape::Bidi];

/// Unary calls and events alone, UDP's.
pub const UNARY_ONLY: &[Shape] = &[Shape::Unary];

impl Capabilities {
    /// A link delivering as `delivery`, carrying every shape, speaking JSON, unordered, with no
    /// frame limit, and holding every ability a `bool` accessor names.
    ///
    /// ```
    /// use ulo_rpc::{Capabilities, DeliveryMode};
    ///
    /// let declared = Capabilities::new(DeliveryMode::Competing);
    /// assert!(!declared.binary);
    /// let held = [
    ///     declared.native_backpressure(),
    ///     declared.miss_signal(),
    ///     declared.unserved_hold(),
    ///     declared.durable_replies(),
    ///     declared.confirmed_drain(),
    ///     declared.ordered_control(),
    ///     declared.single_delivery(),
    ///     declared.every_instance(),
    /// ];
    /// assert_eq!(held, [true; 8]);
    /// ```
    pub const fn new(delivery: DeliveryMode) -> Self {
        Capabilities {
            binary: false,
            max_frame: None,
            ordering: Ordering::Unordered,
            shapes: ALL_SHAPES,
            delivery,
            native_backpressure: None,
            miss_signal: None,
            unserved_hold: None,
            durable_replies: None,
            confirmed_drain: None,
            ordered_control: None,
            single_delivery: None,
            every_instance: None,
        }
    }

    pub const fn binary(self, binary: bool) -> Self {
        Capabilities { binary, ..self }
    }

    pub const fn max_frame(self, max_frame: Option<u64>) -> Self {
        Capabilities { max_frame, ..self }
    }

    pub const fn ordering(self, ordering: Ordering) -> Self {
        Capabilities { ordering, ..self }
    }

    pub const fn shapes(self, shapes: &'static [Shape]) -> Self {
        Capabilities { shapes, ..self }
    }

    /// Sets the `binary` field from the codec the link is configured with: `true` for CBOR,
    /// `false` for JSON.
    ///
    /// ```
    /// use ulo_rpc::{Capabilities, Codec, DeliveryMode};
    ///
    /// assert!(!Capabilities::new(DeliveryMode::Addressed).codec(Codec::Json).binary);
    /// assert!(Capabilities::new(DeliveryMode::Addressed).codec(Codec::Cbor).binary);
    /// ```
    pub const fn codec(self, codec: Codec) -> Self {
        Capabilities { binary: matches!(codec, Codec::Cbor), ..self }
    }

    /// The broker paces deliveries itself, AMQP through its prefetch and Kafka by pausing
    /// partitions: over `Server::max_inflight` the link stops taking requests, which wait in the
    /// broker. On a link without it the server refuses a request over the bound `unavailable`
    /// with a `RetryAfter` detail.
    pub const fn native_backpressure(&self) -> bool {
        self.native_backpressure.is_none()
    }

    pub const fn without_native_backpressure(self, reason: &'static str) -> Self {
        Capabilities { native_backpressure: Some(reason), ..self }
    }

    pub const fn why_not_native_backpressure(&self) -> Option<&'static str> {
        self.native_backpressure
    }

    /// A request to a pattern nothing subscribes to is reported to the caller, which then answers
    /// `Unavailable` with `reason: "no_destination"`; without it the caller's own `Timeout`.
    pub const fn miss_signal(&self) -> bool {
        self.miss_signal.is_none()
    }

    pub const fn without_miss_signal(self, reason: &'static str) -> Self {
        Capabilities { miss_signal: Some(reason), ..self }
    }

    pub const fn why_not_miss_signal(&self) -> Option<&'static str> {
        self.miss_signal
    }

    /// The broker keeps a request for a pattern a server has subscribed while no instance consumes
    /// it, and hands it to the next instance: an AMQP queue, a Kafka topic. A caller reaching a
    /// draining or stopped server then sees its own `Timeout`, and `miss_signal` covers only a
    /// pattern never subscribed.
    pub const fn unserved_hold(&self) -> bool {
        self.unserved_hold.is_none()
    }

    pub const fn without_unserved_hold(self, reason: &'static str) -> Self {
        Capabilities { unserved_hold: Some(reason), ..self }
    }

    pub const fn why_not_unserved_hold(&self) -> Option<&'static str> {
        self.unserved_hold
    }

    /// The broker keeps a reply published while the client's connection is down, and the client
    /// reads it once it reconnects: a Kafka reply topic. A call waiting when the client loses its
    /// connection is then answered, rather than failed `Unavailable` as on a link whose replies
    /// the outage loses.
    pub const fn durable_replies(&self) -> bool {
        self.durable_replies.is_none()
    }

    pub const fn without_durable_replies(self, reason: &'static str) -> Self {
        Capabilities { durable_replies: Some(reason), ..self }
    }

    pub const fn why_not_durable_replies(&self) -> Option<&'static str> {
        self.durable_replies
    }

    /// The link's drain returns only once nothing more is on its way to the server, so every
    /// request that reached it is answered, `Unavailable` once it is draining. A link without it
    /// can lose a request already on its way: the NATS client removes a drained subscription
    /// before the server has processed the unsubscribe and discards what arrives for it
    /// afterwards, and a datagram reaching a UDP server after its socket closes is dropped. A
    /// caller reaching a draining server can then see its own `Timeout` rather than `Unavailable`;
    /// once the server has closed, a miss is reported as on any link with `miss_signal`.
    pub const fn confirmed_drain(&self) -> bool {
        self.confirmed_drain.is_none()
    }

    pub const fn without_confirmed_drain(self, reason: &'static str) -> Self {
        Capabilities { confirmed_drain: Some(reason), ..self }
    }

    pub const fn why_not_confirmed_drain(&self) -> Option<&'static str> {
        self.confirmed_drain
    }

    /// A request and its control frames travel one ordered lane end to end, so a `cancel` sent
    /// after its request reaches the server after it: TCP's connection, Redis's one publisher and
    /// one Pub/Sub connection. On a broker whose control frames travel a lane of their own the
    /// broker can deliver a `cancel` first, and the server drops a `cancel` naming no call it
    /// holds.
    pub const fn ordered_control(&self) -> bool {
        self.ordered_control.is_none()
    }

    pub const fn without_ordered_control(self, reason: &'static str) -> Self {
        Capabilities { ordered_control: Some(reason), ..self }
    }

    pub const fn why_not_ordered_control(&self) -> Option<&'static str> {
        self.ordered_control
    }

    /// A request, and an event left at [`Instances::One`], reaches one server instance however
    /// many listen on its pattern. A link without it hands each such message to every instance
    /// subscribed to its pattern: each runs the handler, a request's handler replies, and the
    /// client answers the first reply and drops the rest, so the handler's side effects run once
    /// per instance. Such a link supports one server instance for those patterns, and
    /// `Server::prepare` logs a `warn` saying so, naming
    /// [`why_not_single_delivery`](Self::why_not_single_delivery), when it mounts a request
    /// handler or an event handler left at its default; it refuses nothing, since one instance
    /// cannot tell itself from many. An event declared `every_instance` is outside it.
    ///
    /// ```
    /// use ulo_rpc::{Capabilities, DeliveryMode};
    ///
    /// let once = Capabilities::new(DeliveryMode::Competing);
    /// assert!(once.single_delivery() && once.why_not_single_delivery().is_none());
    /// let fanned = once.without_single_delivery("Pub/Sub hands a message to every subscriber");
    /// assert!(!fanned.single_delivery());
    /// assert_eq!(fanned.why_not_single_delivery(), Some("Pub/Sub hands a message to every subscriber"));
    /// ```
    pub const fn single_delivery(&self) -> bool {
        self.single_delivery.is_none()
    }

    pub const fn without_single_delivery(self, reason: &'static str) -> Self {
        Capabilities { single_delivery: Some(reason), ..self }
    }

    pub const fn why_not_single_delivery(&self) -> Option<&'static str> {
        self.single_delivery
    }

    /// An event whose handler is declared `every_instance` ([`Instances::Every`]) reaches every
    /// server instance listening on its pattern. A link without it, where a caller addresses one
    /// server, has no other instance to reach: `Server::prepare` refuses such a handler, naming
    /// [`why_not_every_instance`](Self::why_not_every_instance).
    ///
    /// ```
    /// use ulo_rpc::{Capabilities, DeliveryMode};
    ///
    /// let addressed = Capabilities::new(DeliveryMode::Addressed)
    ///     .without_every_instance("the caller addresses one server");
    /// assert!(!addressed.every_instance());
    /// assert_eq!(addressed.why_not_every_instance(), Some("the caller addresses one server"));
    /// ```
    pub const fn every_instance(&self) -> bool {
        self.every_instance.is_none()
    }

    pub const fn without_every_instance(self, reason: &'static str) -> Self {
        Capabilities { every_instance: Some(reason), ..self }
    }

    pub const fn why_not_every_instance(&self) -> Option<&'static str> {
        self.every_instance
    }
}

/// How a request reaches two server instances listening on one pattern; an event's reach is its
/// handler's [`Instances`].
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DeliveryMode {
    /// One instance takes each request: AMQP queues, Kafka consumer groups, a NATS queue group, an
    /// MQTT shared subscription.
    Competing,
    /// Every instance receives each request: Redis Pub/Sub; the client drops a second reply.
    FanOut,
    /// The caller names the server: TCP and UDP.
    Addressed,
}

/// The order a link keeps.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Ordering {
    /// No order: UDP.
    Unordered,
    /// Per connection: TCP.
    PerConnection,
    /// Per publisher and subject: NATS.
    PerPublisher,
    /// Per channel: Redis.
    PerChannel,
    /// Per queue with a single consumer: AMQP.
    PerQueue,
    /// Per topic and QoS: MQTT.
    PerTopic,
    /// Per partition: Kafka.
    PerPartition,
}

/// A link's report that nothing listens on the pattern a request named: NATS no-responders, a
/// Redis receiver count of zero, an AMQP `basic.return`, an MQTT PUBACK or PUBREC with reason
/// 0x10. `RpcClient` answers it `Unavailable` with `reason: "no_destination"`.
#[derive(Debug)]
pub struct NoDestination {
    pub pattern: String,
}

impl fmt::Display for NoDestination {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "nothing listens on pattern `{}`", self.pattern)
    }
}

impl Error for NoDestination {}

/// A server link's refusal to listen on an app that handed it no `Timer`: each link times its
/// close or its first connect on the app's clock (RabbitMQ only its connect) and has none of its
/// own to fall back on. `link` is the transport's
/// name as it reads in prose (`TCP`, `UDP`, `NATS`, `MQTT`, `RabbitMQ`, `Kafka`). A caller finds it with
/// `downcast_ref` on the `BoxError` `listen` returns.
#[derive(Debug)]
pub struct NoTimer {
    pub link: &'static str,
}

impl fmt::Display for NoTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} link's server has no `Timer` to time its close or its connect on: its app has none", self.link)
    }
}

impl Error for NoTimer {}

/// A broker that did not answer within the link's `connect_within`: one that accepts the
/// connection and never completes the handshake, or, on RabbitMQ, one lost between a server's two
/// connections. A server's `listen` fails with it as `Bind`, and `RpcClient` answers it
/// `Unavailable` with `reason: "connect_timed_out"`. It has no source: the bound ran out while the
/// connect was still waiting, with no attempt's error to report.
///
/// A caller finds it with `downcast_ref` on the `BoxError` a link returns:
///
/// ```
/// use std::time::Duration;
/// use ulo::BoxError;
/// use ulo_rpc::ConnectTimedOut;
///
/// let failed: BoxError = ConnectTimedOut::new(Duration::from_secs(30), "10.0.0.7:4222", "NATS").into();
/// let timed_out = failed.downcast_ref::<ConnectTimedOut>().expect("the link's connect ran out");
/// assert_eq!((timed_out.within(), timed_out.address(), timed_out.link()), (Duration::from_secs(30), "10.0.0.7:4222", "NATS"));
/// assert_eq!(failed.to_string(), "the NATS broker at 10.0.0.7:4222 did not answer within the link's `connect_within` of 30s");
/// ```
#[derive(Debug)]
pub struct ConnectTimedOut {
    within: Duration,
    address: String,
    link: &'static str,
}

impl ConnectTimedOut {
    /// `link` is the transport's name as it reads in prose (`NATS`, `RabbitMQ`), and `address` the
    /// broker's `host:port`, given without the url's credentials, since the error is shown whole.
    pub fn new(within: Duration, address: impl Into<String>, link: &'static str) -> Self {
        ConnectTimedOut { within, address: address.into(), link }
    }

    /// The bound that ran out.
    pub fn within(&self) -> Duration {
        self.within
    }

    /// The broker's `host:port`.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// The transport's name as it reads in prose.
    pub fn link(&self) -> &'static str {
        self.link
    }
}

impl fmt::Display for ConnectTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} broker at {} did not answer within the link's `connect_within` of {:?}", self.link, self.address, self.within)
    }
}

impl Error for ConnectTimedOut {}

/// A link's refusal to connect over a misconfiguration only its connect could see: an `mqtts://`
/// or `tls://` link whose platform root certificates did not load, a NATS link whose roots file
/// could not be written, a `tls://` NATS link in a process that links more than one rustls crypto
/// provider and installs none, or a `tls://` NATS link whose roots' load outlasted its
/// `roots_timeout`. No retry mends the first three, whose cause the link keeps; the last is mended
/// by a later call once the load has ended, the load running on past the bound. `RpcClient`
/// answers it `Internal` with `reason: "misconfigured_link"`, not the `Unavailable` of a connect
/// that failed, and asks again on the next call, which a link holding the cause answers at once.
#[derive(Debug)]
pub struct Misconfigured {
    pub source: BoxError,
}

impl fmt::Display for Misconfigured {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.source, f)
    }
}

impl Error for Misconfigured {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

/// A frame over the link's limit, refused before it is sent. `RpcClient` answers it `BadRequest`
/// with `reason: "payload_too_large"`.
#[derive(Debug)]
pub struct FrameTooLarge {
    pub size: u64,
    pub limit: u64,
}

impl fmt::Display for FrameTooLarge {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a frame of {} bytes is over the link's limit of {} bytes", self.size, self.limit)
    }
}

impl Error for FrameTooLarge {}

/// A frame the link's codec cannot encode, from [`Codec::encode_frame`]: a payload that is not an
/// item of that codec, such as a `Data` built from JSON bytes and answered on a CBOR link. On a
/// reply lane it is the server's own failure, which the server answers with an `err` of kind
/// `internal` and logs at `error`; `RpcClient` answers it `Internal` on a request.
#[derive(Debug)]
pub struct FrameUnencodable {
    pub codec: Codec,
    pub source: BoxError,
}

impl fmt::Display for FrameUnencodable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {:?} codec cannot encode the frame: {}", self.codec, self.source)
    }
}

impl Error for FrameUnencodable {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

/// A panic a link caught where no task boundary converts it, as the redacted message a task's
/// panic answers: `payload` raised again in a task on `runtime` and caught by the core there, so
/// `ulo::is_panic` finds it on an error that keeps it as its `source()`. For a link that runs work
/// on a blocking thread or a library callback, where a panic reaches it as a payload rather than
/// as a task's end: the NATS and MQTT links' load of their root certificates. The raise is a
/// `resume_unwind`, which runs no panic hook, so the panic is not printed a second time. `None`
/// when `runtime` drops the task unrun, having shut down.
pub async fn panic_redacted<R: ulo::Spawn + ?Sized>(runtime: &R, payload: Box<dyn std::any::Any + Send>) -> Option<Arc<ulo::Redacted>> {
    let raised = ulo::spawn_with(runtime, async move {
        std::panic::resume_unwind(payload);
    });
    match raised.await {
        Err(ulo::TaskEnd::Panicked(message)) => Some(message),
        _ => None,
    }
}

/// Where the long-lived tasks of one side of a link report a panic: a server's lanes and writer, a
/// client's writer and reply router, an event loop. A link spawns such a task on a runtime of its
/// own and holds it through an abort handle, so its end reaches nothing; spawned as
/// [`watch`](Self::watch) or [`watch_feeder`](Self::watch_feeder) wraps it, a panic is kept, and
/// the side's stream raises it at its next poll, through [`guard`](Self::guard). The text raised
/// names the link and the task, `the NATS link's request lane panicked: …`, and is redacted where
/// it is caught:
///
/// - a server's inbound stream raises it inside `ulo_rpc::Server::serve`, whose panic the app
///   catches at each poll, so the shutdown begins as for a transport failure and its outcome is
///   `ServeEnd::Panicked` holding the message with the app's secrets replaced, `ulo::is_panic`
///   true of it;
/// - a client's reply lane raises it inside `RpcClient`'s reply router, which fails every call
///   waiting on the connection `Unavailable` holding the panic redacted on the client's runtime,
///   `ulo::is_panic` true of the call's error, and connects again for the next call;
/// - a client's send that fails answers its failure through [`explain`](Self::explain), which
///   answers the panic in its place: a writer holding a send's answer until its broker settles it
///   drops the answer as it unwinds, before its panic is kept, so the send would otherwise fail
///   on a closed client.
///
/// A panic kept when no stream is guarded, or after one has been raised, is logged at `error`, its
/// message redacted on `runtime` as a task's panic is: the app's runtime redacts with the app's
/// secrets, a runtime outside any app strips a url's userinfo alone. A panic that unwinds out of
/// a watched task runs the process's panic hook first, which prints the unredacted message unless
/// the application installs one of its own; this type governs what the link reports.
///
/// ```
/// use std::sync::Arc;
/// use futures_util::StreamExt;
/// use ulo_rpc::link::TaskPanics;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() {
/// let runtime = Arc::new(ulo_runtime_tokio::Tokio::current());
/// let panics = TaskPanics::new("Example", runtime);
/// let (frames, received) = futures_channel::mpsc::unbounded::<u32>();
/// let mut inbound = panics.guard(received);
/// let lane = tokio::spawn(panics.watch_feeder("request lane", async move {
///     let _frames = frames;
///     panic!("the lane failed");
/// }));
/// lane.await.expect("a watched task completes, its panic kept");
/// let raised = std::panic::AssertUnwindSafe(inbound.next()).catch_unwind().await;
/// let payload = raised.err().expect("the guarded stream raises the kept panic");
/// assert_eq!(payload.downcast_ref::<String>().map(String::as_str), Some("the Example link's request lane panicked: the lane failed"));
/// # }
/// # use futures_util::FutureExt;
/// ```
#[derive(Clone)]
pub struct TaskPanics(Arc<Panics>);

struct Panics {
    /// The transport's name as it reads in prose, `NATS`.
    link: &'static str,
    runtime: Arc<dyn Runtime>,
    state: Mutex<PanicState>,
    /// Each watched task's poll count, for [`TaskPanics::explain`]'s wait.
    slots: Mutex<Vec<Weak<Slot>>>,
    /// Notified each time a watched task leaves a poll.
    left: event_listener::Event,
    #[cfg(feature = "conformance-probe")]
    planted: Mutex<Option<PlantedPanic>>,
    /// The watched tasks not yet dropped, for a link's own tests.
    #[cfg(feature = "conformance-probe")]
    live: Mutex<Option<Arc<std::sync::atomic::AtomicUsize>>>,
}

/// One watched task counted live, until its future drops.
#[cfg(feature = "conformance-probe")]
struct Live(Option<Arc<std::sync::atomic::AtomicUsize>>);

#[cfg(feature = "conformance-probe")]
impl Drop for Live {
    fn drop(&mut self) {
        if let Some(live) = &self.0 {
            live.fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        }
    }
}

#[derive(Default)]
struct PanicState {
    /// A stream is guarded and has raised nothing.
    guarded: bool,
    /// The panic the guarded stream raises at its next poll, with its task's name.
    kept: Option<(&'static str, String)>,
    /// The guarded stream's reader, woken when a panic is kept or the last feeder ends.
    waker: Option<std::task::Waker>,
    /// Feeders whose watch has not ended.
    feeders: usize,
    /// The first panic any watched task raised, kept after it is raised or logged, for
    /// [`TaskPanics::explain`].
    first: Option<String>,
}

/// One watched task's polls: odd while a poll is under way, even between polls.
#[derive(Default)]
struct Slot {
    polls: std::sync::atomic::AtomicU64,
}

/// A watched task under its catch: each poll is counted in its [`Slot`], and the slot leaves the
/// poll only once a panic in it is kept, so a wait for the poll's end finds the panic. Answers the
/// panic's text when nothing kept it, to log.
struct Caught<F> {
    /// `None` once it has ended.
    task: Option<std::pin::Pin<Box<F>>>,
    panics: Arc<Panics>,
    name: &'static str,
    slot: Arc<Slot>,
}

impl<F: Future<Output = ()>> Future for Caught<F> {
    type Output = Option<String>;

    fn poll(self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<String>> {
        use std::sync::atomic::Ordering::AcqRel;
        use std::task::Poll;
        let this = self.get_mut();
        let Some(task) = this.task.as_mut() else { return Poll::Ready(None) };
        this.slot.polls.fetch_add(1, AcqRel);
        let polled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task.as_mut().poll(cx)));
        let answer = match polled {
            Ok(Poll::Pending) => Poll::Pending,
            Ok(Poll::Ready(())) => Poll::Ready(None),
            Err(payload) => {
                let text = format!("the {} link's {} panicked: {}", this.panics.link, this.name, payload_text(&*payload));
                let kept = this.panics.keep(this.name, text);
                drop_caught(payload);
                Poll::Ready(kept)
            }
        };
        if answer.is_ready() {
            // Its state, dropped as the panic unwound, or ended: the future is not polled again.
            drop_caught(this.task.take());
        }
        this.slot.polls.fetch_add(1, AcqRel);
        this.panics.left.notify(usize::MAX);
        answer
    }
}

/// Drops `value` under a catch, inside the poll that [`Caught`] counts: a drop that panics, a
/// panic payload's own among them, would otherwise unwind out of the poll with its slot still odd,
/// and [`TaskPanics::explain`] would wait on it forever. The caught payload is forgotten, since
/// its drop could panic again.
fn drop_caught<T>(value: T) {
    if let Err(payload) = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || drop(value))) {
        std::mem::forget(payload);
    }
}

/// A watched task's panic, as [`TaskPanics::explain`] answers it: its `source()` is the redacted
/// panic, which `ulo::is_panic` finds.
#[derive(Debug)]
struct TaskPanicked(Arc<ulo::Redacted>);

impl fmt::Display for TaskPanicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl Error for TaskPanicked {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.0)
    }
}

/// One feeder counted, until its watch drops.
struct Feeding(Arc<Panics>);

impl Drop for Feeding {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.feeders -= 1;
        if state.feeders == 0
            && let Some(waker) = state.waker.take()
        {
            waker.wake();
        }
    }
}

impl TaskPanics {
    /// The panics of one side of the `link` link, `link` the transport's name as it reads in prose,
    /// whose messages a log line redacts on `runtime`: the app's for a server's side, from
    /// `AppHandle::runtime`, and the client's, from [`Link::connect`], for a client's.
    pub fn new(link: &'static str, runtime: Arc<dyn Runtime>) -> Self {
        TaskPanics(Arc::new(Panics {
            link,
            runtime,
            state: Mutex::new(PanicState::default()),
            slots: Mutex::new(Vec::new()),
            left: event_listener::Event::new(),
            #[cfg(feature = "conformance-probe")]
            planted: Mutex::new(None),
            #[cfg(feature = "conformance-probe")]
            live: Mutex::new(None),
        }))
    }

    /// `task` under a catch, to spawn in its place: a panic in it ends it, and is kept for the
    /// guarded stream to raise, or logged. `name` says which task it is in the text, `writer`,
    /// `event loop`. A task holding a sender of the guarded stream is a
    /// [`watch_feeder`](Self::watch_feeder).
    pub fn watch<F>(&self, name: &'static str, task: F) -> impl Future<Output = ()> + Send + 'static + use<F>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.watched(name, task, None)
    }

    /// [`watch`](Self::watch) for a task holding a sender of the guarded stream, a lane or a reply
    /// router: the stream's end waits for the task's end, so a panic that drops the stream's last
    /// sender, while it unwinds and before it is kept, is raised rather than read as the stream's
    /// end. Counted from this call, so a feeder spawned after the stream is guarded is counted
    /// before the stream can end.
    pub fn watch_feeder<F>(&self, name: &'static str, task: F) -> impl Future<Output = ()> + Send + 'static + use<F>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).feeders += 1;
        self.watched(name, task, Some(Feeding(Arc::clone(&self.0))))
    }

    fn watched<F>(&self, name: &'static str, task: F, feeding: Option<Feeding>) -> impl Future<Output = ()> + Send + 'static + use<F>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let panics = Arc::clone(&self.0);
        #[cfg(feature = "conformance-probe")]
        let planted = panics.planted.lock().unwrap_or_else(PoisonError::into_inner).clone().filter(|planted| planted.task == name);
        #[cfg(feature = "conformance-probe")]
        let live = {
            let live = panics.live.lock().unwrap_or_else(PoisonError::into_inner).clone();
            if let Some(live) = &live {
                live.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            }
            Live(live)
        };
        let slot = Arc::new(Slot::default());
        panics.slots.lock().unwrap_or_else(PoisonError::into_inner).push(Arc::downgrade(&slot));
        async move {
            // Dropped with this future, at its end or its abort, after the panic is kept.
            let _feeding = feeding;
            #[cfg(feature = "conformance-probe")]
            let _live = live;
            #[cfg(feature = "conformance-probe")]
            let task = async move {
                let Some(planted) = planted else { return task.await };
                let fired = async move {
                    planted.fired.wait_for(|fired| *fired).await;
                    panic!("{}", planted.message);
                };
                futures_util::future::select(std::pin::pin!(task), std::pin::pin!(fired)).await;
            };
            let caught = Caught { task: Some(Box::pin(task)), panics: Arc::clone(&panics), name, slot };
            if let Some(unraised) = caught.await {
                panics.log(name, unraised).await;
            }
        }
    }

    /// `error`, a send's failure on this side, or the panic of one of its watched tasks when one
    /// has panicked: a writer that panics drops the answers it holds while it unwinds, before its
    /// panic is kept, so a send can fail on the dropped answer first. The wait first lets every
    /// watched task that is inside a poll leave it, so a panic that dropped the answer is kept by
    /// then; no clock is read. The panic answers as an error whose `source()` is the panic, its
    /// message redacted on the side's runtime, so `ulo::is_panic` is true of it. It is the side's
    /// first panic, whatever failed the send, and `error` answers in its place once the side's
    /// runtime has shut down, the panic then having no redaction.
    pub async fn explain(&self, error: BoxError) -> BoxError {
        self.0.polls_ended().await;
        let first = self.0.state.lock().unwrap_or_else(PoisonError::into_inner).first.clone();
        let Some(text) = first else { return error };
        match panic_redacted(&*self.0.runtime, Box::new(text)).await {
            Some(message) => Box::new(TaskPanicked(message)),
            None => error,
        }
    }

    /// `stream`, raising a kept panic at its next poll, its payload the text naming the link and
    /// the task; called once per side, on the stream the side hands to the server or the client.
    /// Dropped with a panic kept and not raised, it logs the panic.
    pub fn guard<S>(&self, stream: S) -> BoxStream<'static, S::Item>
    where
        S: futures_core::Stream + Send + 'static,
    {
        self.0.state.lock().unwrap_or_else(PoisonError::into_inner).guarded = true;
        Box::pin(Guarded { stream: Box::pin(stream), panics: Arc::clone(&self.0), ended: false })
    }

    /// The watched tasks `planted` names panic once it fires, for a link's own tests, which have no
    /// other way to make a lane panic.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn plant(&self, planted: Option<PlantedPanic>) {
        *self.0.planted.lock().unwrap_or_else(PoisonError::into_inner) = planted;
    }

    /// Counts every task watched from here into `live` while its future lives, for a link's own
    /// tests: a task that ends, or is aborted and dropped, leaves the count.
    #[doc(hidden)]
    #[cfg(feature = "conformance-probe")]
    pub fn count_into(&self, live: Arc<std::sync::atomic::AtomicUsize>) {
        *self.0.live.lock().unwrap_or_else(PoisonError::into_inner) = Some(live);
    }
}

/// A panic a link's test plants in the tasks of one name, `request lane` or `writer`, through its
/// link's `panic_in`: each such task, spawned under [`TaskPanics`], panics with `message` once
/// [`fire`](Self::fire) has been called, at its first poll when it was before. Test support under
/// `conformance-probe`, outside the semver promise.
#[doc(hidden)]
#[cfg(feature = "conformance-probe")]
#[derive(Clone)]
pub struct PlantedPanic {
    task: &'static str,
    message: &'static str,
    fired: Arc<ulo_transport::sync::Watch<bool>>,
}

#[cfg(feature = "conformance-probe")]
impl PlantedPanic {
    pub fn new(task: &'static str, message: &'static str) -> Self {
        PlantedPanic { task, message, fired: Arc::new(ulo_transport::sync::Watch::new(false)) }
    }

    pub fn fire(&self) {
        self.fired.modify(|fired| *fired = true);
    }
}

impl Panics {
    /// Resolves once every watched task that was inside a poll when it was called has left that
    /// poll. Each slot's count changes as its poll ends, so a task polled again later does not hold
    /// the wait.
    async fn polls_ended(&self) {
        use std::sync::atomic::Ordering::Acquire;
        let polling: Vec<(Arc<Slot>, u64)> = {
            let mut slots = self.slots.lock().unwrap_or_else(PoisonError::into_inner);
            slots.retain(|slot| slot.strong_count() > 0);
            slots
                .iter()
                .filter_map(Weak::upgrade)
                .filter_map(|slot| {
                    let polls = slot.polls.load(Acquire);
                    (polls % 2 == 1).then_some((slot, polls))
                })
                .collect()
        };
        loop {
            let left = self.left.listen();
            if polling.iter().all(|(slot, at)| slot.polls.load(Acquire) != *at) {
                return;
            }
            left.await;
        }
    }

    /// Keeps `text` for the guarded stream, waking its reader; answers it back when no stream is
    /// guarded or one panic is already kept or raised.
    fn keep(&self, task: &'static str, text: String) -> Option<String> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.first.get_or_insert_with(|| text.clone());
        if !state.guarded || state.kept.is_some() {
            return Some(text);
        }
        state.kept = Some((task, text));
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
        None
    }

    async fn log(&self, task: &'static str, text: String) {
        match panic_redacted(&*self.runtime, Box::new(text)).await {
            Some(message) => tracing::error!(link = self.link, task, %message, "a task of the {} link panicked and was not raised to its server or client", self.link),
            None => tracing::error!(
                link = self.link,
                task,
                "a task of the {} link panicked; its message is not logged, the runtime that redacts it having shut down",
                self.link
            ),
        }
    }
}

/// What [`TaskPanics::guard`] answers.
struct Guarded<T> {
    stream: BoxStream<'static, T>,
    panics: Arc<Panics>,
    /// The inner stream answered its end, and is not polled again.
    ended: bool,
}

impl<T> futures_core::Stream for Guarded<T> {
    type Item = T;

    fn poll_next(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Option<T>> {
        use std::task::Poll;
        if !self.ended {
            self.panics.raise_kept(cx, false);
            match self.stream.as_mut().poll_next(cx) {
                Poll::Ready(None) => self.ended = true,
                other => return other,
            }
        }
        // A feeder's panic drops the stream's last sender while it unwinds, before the panic is
        // kept, so the end waits for every feeder's watch to end.
        if self.panics.raise_kept(cx, true) { Poll::Pending } else { Poll::Ready(None) }
    }
}

impl Panics {
    /// Raises the kept panic, if there is one. Otherwise registers `cx`'s waker for the next panic
    /// kept, and answers whether a feeder is still counted, the waker then woken at its end too;
    /// `ending` registers the waker only while one is.
    fn raise_kept(&self, cx: &mut std::task::Context<'_>, ending: bool) -> bool {
        let kept = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            match state.kept.take() {
                Some(kept) => {
                    state.guarded = false;
                    kept
                }
                None => {
                    let feeding = state.feeders > 0;
                    if feeding || !ending {
                        state.waker = Some(cx.waker().clone());
                    }
                    return feeding;
                }
            }
        };
        std::panic::resume_unwind(Box::new(kept.1));
    }
}

impl<T> Drop for Guarded<T> {
    fn drop(&mut self) {
        let kept = {
            let mut state = self.panics.state.lock().unwrap_or_else(PoisonError::into_inner);
            state.guarded = false;
            state.waker = None;
            state.kept.take()
        };
        if let Some((task, text)) = kept {
            let panics = Arc::clone(&self.panics);
            drop(self.panics.runtime.spawn(Box::pin(async move { panics.log(task, text).await })));
        }
    }
}

/// A panic payload's message, as the core reads one.
fn payload_text(payload: &(dyn std::any::Any + Send)) -> &str {
    if let Some(text) = payload.downcast_ref::<&'static str>() {
        text
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text
    } else {
        "a panic whose payload is not a string"
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::task::{Context, Poll, Waker};

    use futures_util::{FutureExt, StreamExt};

    use super::*;

    type Guarded = Arc<Mutex<Option<BoxStream<'static, u32>>>>;

    /// Drops the guarded stream's last sender while the feeder holding it unwinds, then polls the
    /// stream once, recording whether it answered its end, as a reader on another thread could at
    /// that moment.
    struct Probe {
        guarded: Guarded,
        ended: Arc<Mutex<Option<bool>>>,
        sender: Option<futures_channel::mpsc::UnboundedSender<u32>>,
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            drop(self.sender.take());
            let mut guarded = self.guarded.lock().unwrap_or_else(PoisonError::into_inner);
            let stream = guarded.as_mut().expect("the guarded stream");
            let mut cx = Context::from_waker(Waker::noop());
            let polled = catch_unwind(AssertUnwindSafe(|| stream.poll_next_unpin(&mut cx)));
            *self.ended.lock().unwrap_or_else(PoisonError::into_inner) = Some(matches!(polled, Ok(Poll::Ready(None))));
        }
    }

    /// A feeder whose panic drops the stream's last sender while it unwinds, before the panic is
    /// kept, does not end the stream: a poll in that moment waits, and the next poll, once the
    /// feeder's watch has ended, raises the panic.
    #[tokio::test(flavor = "current_thread")]
    async fn a_feeder_s_panic_is_raised_and_not_read_as_the_stream_s_end() {
        let panics = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        let (sender, received) = futures_channel::mpsc::unbounded::<u32>();
        let guarded: Guarded = Arc::new(Mutex::new(Some(panics.guard(received))));
        let ended = Arc::new(Mutex::new(None));
        let probe = Probe { guarded: Arc::clone(&guarded), ended: Arc::clone(&ended), sender: Some(sender) };
        panics
            .watch_feeder("lane", async move {
                let _probe = probe;
                tokio::task::yield_now().await;
                panic!("the lane failed");
            })
            .await;
        assert_eq!(*ended.lock().unwrap_or_else(PoisonError::into_inner), Some(false), "the unwinding feeder's dropped sender ended the stream");
        let mut stream = guarded.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the guarded stream");
        let raised = AssertUnwindSafe(stream.next()).catch_unwind().await.err().expect("the stream raised the feeder's panic");
        assert_eq!(raised.downcast_ref::<String>().map(String::as_str), Some("the Test link's lane panicked: the lane failed"));
    }

    /// Drops a send's answer while the writer holding it unwinds, then starts the send's
    /// [`TaskPanics::explain`] there, as a caller on another thread could at that moment, keeping
    /// the wait for the test.
    struct Answer {
        answer: Option<futures_channel::oneshot::Sender<()>>,
        panics: TaskPanics,
        /// The wait, and whether its first poll, inside the unwinding, was still waiting.
        explained: Arc<Mutex<Option<(BoxFuture<'static, BoxError>, bool)>>>,
    }

    impl Drop for Answer {
        fn drop(&mut self) {
            drop(self.answer.take());
            let panics = self.panics.clone();
            let mut explaining: BoxFuture<'static, BoxError> = Box::pin(async move { panics.explain("the link's client is closed".into()).await });
            let mut cx = Context::from_waker(Waker::noop());
            let waiting = explaining.as_mut().poll(&mut cx).is_pending();
            *self.explained.lock().unwrap_or_else(PoisonError::into_inner) = Some((explaining, waiting));
        }
    }

    /// A send that fails on an answer its writer dropped while panicking is told the panic, not
    /// the closed client: the wait started inside the writer's unwinding ends once its poll does,
    /// with the panic kept, and answers it redacted, `ulo::is_panic` true of it. A send failing
    /// with no panic keeps its own error.
    #[tokio::test(flavor = "current_thread")]
    async fn a_send_failing_on_a_panicked_writer_s_dropped_answer_is_told_the_panic() {
        let panics = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        let (answer, _answered) = futures_channel::oneshot::channel();
        let explained = Arc::new(Mutex::new(None));
        let held = Answer { answer: Some(answer), panics: panics.clone(), explained: Arc::clone(&explained) };
        panics
            .watch("writer", async move {
                let _held = held;
                tokio::task::yield_now().await;
                panic!("the writer failed with redis://ulo:hunter2@cache");
            })
            .await;
        let (explaining, waiting) = explained.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the send's wait");
        assert!(waiting, "the send's error was explained inside the writer's unwinding, before its panic was kept");
        let error = explaining.await;
        assert!(ulo::is_panic(&error), "the send's error does not hold the panic: {error}");
        assert_eq!(error.to_string(), "the Test link's writer panicked: the writer failed with redis://[redacted]@cache");

        let calm = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        let error = calm.explain("the link's client is closed".into()).await;
        assert!(!ulo::is_panic(&error), "a send with no panic was told one");
        assert_eq!(error.to_string(), "the link's client is closed");
    }

    /// A panic payload whose drop starts a send's [`TaskPanics::explain`], as a caller on another
    /// thread could at that moment, then panics itself.
    struct Payload {
        panics: TaskPanics,
        explained: Arc<Mutex<Option<BoxFuture<'static, BoxError>>>>,
    }

    impl Drop for Payload {
        fn drop(&mut self) {
            let panics = self.panics.clone();
            let mut explaining: BoxFuture<'static, BoxError> = Box::pin(async move { panics.explain("the link's client is closed".into()).await });
            let _ = explaining.as_mut().poll(&mut Context::from_waker(Waker::noop()));
            *self.explained.lock().unwrap_or_else(PoisonError::into_inner) = Some(explaining);
            panic!("the payload's drop panicked");
        }
    }

    /// A panic whose payload panics as it drops still ends the poll it came from, the task's end
    /// a completion and not a panic, so a send's wait started inside that poll answers the panic.
    #[tokio::test(flavor = "current_thread")]
    async fn a_panic_whose_payload_panics_on_drop_still_ends_its_poll() {
        let panics = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        let explained = Arc::new(Mutex::new(None));
        let payload = Payload { panics: panics.clone(), explained: Arc::clone(&explained) };
        let ended = AssertUnwindSafe(panics.watch("writer", async move {
            tokio::task::yield_now().await;
            std::panic::panic_any(payload);
        }))
        .catch_unwind()
        .await;
        assert!(ended.is_ok(), "the payload's panic unwound out of the watched task");
        let explaining = explained.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the send's wait");
        let error = tokio::time::timeout(std::time::Duration::from_secs(5), explaining).await.expect("the send's wait ended");
        assert!(ulo::is_panic(&error), "the send's error does not hold the panic: {error}");
    }

    /// Records whether it was woken.
    struct Woken(std::sync::atomic::AtomicBool);

    impl std::task::Wake for Woken {
        fn wake(self: Arc<Self>) {
            self.0.store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }

    /// A send's wait is woken as the polls under way at its start end, and ends though their task
    /// is inside a later poll by then: here the task waiting is the watched one, so its own poll is
    /// under way at the start, and the wait answers inside its next poll.
    #[tokio::test(flavor = "current_thread")]
    async fn a_task_inside_a_later_poll_does_not_hold_the_wait() {
        let panics = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        let waiting = panics.clone();
        let woken = Arc::new(Woken(std::sync::atomic::AtomicBool::new(false)));
        let waker = Waker::from(Arc::clone(&woken));
        let (told, answered) = futures_channel::oneshot::channel();
        panics
            .watch("writer", async move {
                let mut explaining = Box::pin(waiting.explain("the link's client is closed".into()));
                let first = explaining.as_mut().poll(&mut Context::from_waker(&waker)).is_pending();
                tokio::task::yield_now().await;
                let second = explaining.as_mut().poll(&mut Context::from_waker(Waker::noop())).map(|error| error.to_string());
                let _ = told.send((first, second));
            })
            .await;
        let (first, second) = answered.await.expect("the task reported");
        assert!(first, "the wait ended while the poll under way at its start was");
        assert!(woken.0.load(std::sync::atomic::Ordering::SeqCst), "the wait was not woken as the poll ended");
        assert_eq!(second, Poll::Ready("the link's client is closed".to_owned()), "a later poll of the same task held the wait");
    }

    /// A stream that ends while no task has panicked ends cleanly, and a panic kept with no stream
    /// guarded is not raised by a stream guarded after it.
    #[tokio::test(flavor = "current_thread")]
    async fn a_clean_end_raises_nothing_and_an_unguarded_panic_is_not_raised_later() {
        let panics = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        let (sender, received) = futures_channel::mpsc::unbounded::<u32>();
        let mut guarded = panics.guard(received);
        panics.watch_feeder("lane", async move { drop(sender) }).await;
        assert_eq!(guarded.next().await, None, "a clean end yielded an item");

        let panics = TaskPanics::new("Test", Arc::new(ulo_runtime_tokio::Tokio::current()));
        panics.watch("lane", async { panic!("before any stream was guarded") }).await;
        let (sender, received) = futures_channel::mpsc::unbounded::<u32>();
        let mut guarded = panics.guard(received);
        drop(sender);
        let polled = AssertUnwindSafe(guarded.next()).catch_unwind().await;
        assert!(matches!(polled, Ok(None)), "a panic kept before the stream was guarded was raised");
    }
}
