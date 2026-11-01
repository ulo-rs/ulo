//! The RPC client (transports DESIGN §5.4): requests, events and the streamed shapes over a link's
//! client side, connected lazily on the first call.
//!
//! ```
//! # use std::time::Duration;
//! # use ulo_rpc::{RpcClient, RpcError};
//! # #[derive(serde::Serialize)] struct NewInvoice { tenant: String }
//! # #[derive(serde::Deserialize)] struct Invoice;
//! async fn create(billing: &RpcClient, order: &NewInvoice) -> Result<Invoice, RpcError> {
//!     billing.request::<_, Invoice>("invoices.create", order)
//!         .header("tenant", order.tenant.as_str())
//!         .timeout(Duration::from_secs(2))
//!         .await
//! }
//! ```
//!
//! One task per connection, spawned on the client's runtime, routes the link's reply lane to the
//! calls waiting on it, by `id`. A reply for an id no call waits on any more, a second reply on a
//! `FanOut` link or one arriving after its call gave up, is dropped. A lost reply lane fails every
//! call waiting on it `Unavailable`, and the next call connects again; a `goaway` lets the calls in
//! flight finish and sends the next call over a new connection.

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::{Future, IntoFuture};
use std::marker::PhantomData;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_channel::mpsc::{self, UnboundedReceiver, UnboundedSender};
use futures_core::Stream;
use futures_core::stream::BoxStream;
use futures_util::{FutureExt, StreamExt};
use serde::Serialize;
use serde::de::DeserializeOwned;
use ulo::{Bound, BoxError, BoxFuture, CancelReason, ExecutionRef, Runtime, Timer, TraceOrigin};
use ulo_transport::{Classify, Detail, Details, ErrorKind};

use crate::codec::Codec;
use crate::dispatch::{quote, reason};
use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{ConnectTimedOut, FrameTooLarge, FrameUnencodable, Link, Misconfigured, NoDestination, Outbound, Pattern, ReplyTo};
use crate::trace;
use crate::transport::CallHeaders;

/// A client's timeout at `Bound::Default`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The header the client writes from a call's bound and the server reads as its deadline. No ulo
/// path copies an incoming call's headers into an outgoing one, so a caller setting it with
/// `.header` is a mistake, refused when the call is awaited.
const DEADLINE_MS: &str = "deadline-ms";

/// The W3C Trace Context headers the client writes from the calling execution's context on a call
/// made `.within` one that carries a context (integrations §6.3, I8).
const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";

fn timeout_of(timeout: Bound) -> Option<Duration> {
    match timeout {
        Bound::Default => Some(DEFAULT_TIMEOUT),
        Bound::After(after) => Some(after),
        Bound::Unbounded => None,
    }
}

/// A client's idle timeout: none unless one is given, since a stream may be quiet for as long as
/// it runs.
fn idle_of(idle: Bound) -> Option<Duration> {
    match idle {
        Bound::After(after) => Some(after),
        Bound::Default | Bound::Unbounded => None,
    }
}

/// A zero timeout given to an RPC client, which would time out every call it applies to:
/// `Bound::After(Duration::ZERO)` refused by [`RpcClient::timeout`] and
/// [`RpcClient::idle_timeout`] where it is written, and by `RpcClientModule::timeout` and
/// `RpcClientModule::idle_timeout` when the app wires. A call's own zero, a stream's
/// [`idle_timeout`](StreamCall::idle_timeout) of `Bound::After(Duration::ZERO)`, a
/// [`Call::timeout`] or a [`StreamCall::deadline`] of `Duration::ZERO`, is refused when the call
/// is awaited, before it connects, as the source of a `BadRequest` [`RpcError`] whose reason is
/// `"zero_timeout"`. `Bound::Unbounded` is the spelling of no timeout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZeroTimeout {
    setter: &'static str,
    /// The zero as the setter takes it: `Bound::After(Duration::ZERO)` or `Duration::ZERO`.
    written: &'static str,
    link: &'static str,
    /// What the timeout would end at once: `"every call"` for a timeout, `"every stream"` for a
    /// client's idle timeout, `"the call"` or `"the stream"` for a call's own.
    ends: &'static str,
    /// What to write instead.
    instead: &'static str,
}

impl ZeroTimeout {
    pub(crate) fn of_module(setter: &'static str, link: &'static str, ends: &'static str) -> Self {
        ZeroTimeout::of_bound(setter, link, ends)
    }

    /// A zero written as `Bound::After(Duration::ZERO)`, where `Bound::Unbounded` is the spelling
    /// of none.
    fn of_bound(setter: &'static str, link: &'static str, ends: &'static str) -> Self {
        ZeroTimeout {
            setter,
            written: "Bound::After(Duration::ZERO)",
            link,
            ends,
            instead: "write `Bound::Unbounded` to turn the timeout off",
        }
    }

    /// A zero written as `Duration::ZERO` on a call, whose setter has no spelling of none.
    fn of_duration(setter: &'static str, link: &'static str, ends: &'static str, instead: &'static str) -> Self {
        ZeroTimeout { setter, written: "Duration::ZERO", link, ends, instead }
    }

    /// The `BadRequest` a call given this zero fails with when it is awaited, before it connects.
    fn refusal(self) -> RpcError {
        RpcError::new(ErrorKind::BadRequest, self.to_string()).with_reason("zero_timeout").with_source(self)
    }

    /// The link's name, `Link::NAME`.
    pub fn link(&self) -> &'static str {
        self.link
    }
}

impl fmt::Display for ZeroTimeout {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}({})` on the {} link would time out {}; {}", self.setter, self.written, self.link, self.ends, self.instead)
    }
}

impl Error for ZeroTimeout {}

/// A link a client cannot use, refused before any call: its [`Link::usable`] failed, a tokio-based
/// link built outside a runtime and given none, or a TCP link given a host name. [`RpcClient::new`]
/// answers it, and `RpcClientModule`'s init hook fails the app's `connect` with it.
#[derive(Debug)]
pub struct UnusableLink {
    site: &'static str,
    link: &'static str,
    source: BoxError,
}

impl UnusableLink {
    pub(crate) fn of_module(link: &'static str, source: BoxError) -> Self {
        UnusableLink { site: "RpcClientModule::for_root", link, source }
    }

    /// The link's name, `Link::NAME`.
    pub fn link(&self) -> &'static str {
        self.link
    }
}

impl fmt::Display for UnusableLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "`{}` was given a {} link it cannot use: {}", self.site, self.link, self.source)
    }
}

impl Error for UnusableLink {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

/// A client of one link, bound by `RpcClientModule` as `Dep<RpcClient, Billing>` under the
/// module's qualifier, or built outside an app with [`RpcClient::new`]. Cheap to clone. Its tasks
/// and every timeout run on the runtime it was given, the app's inside an app; dropping a pending
/// request or a stream sends `cancel`, spawned on that runtime. There is no ambient execution: a
/// call made inside one forwards its deadline and cancellation with [`Call::within`].
///
/// Each bound has one meaning on every call shape. A `timeout` bounds the whole call: on a
/// [`request`](Self::request) or a [`send_stream`](Self::send_stream) it is carried to the server
/// as `deadline-ms`, so the server stops the call when the client gives up on it, and on an
/// [`emit`](Self::emit) it bounds the publish, since nothing comes back. A
/// [`stream`](Self::stream) or a [`duplex`](Self::duplex) call has no `timeout`: its
/// [`idle_timeout`](StreamCall::idle_timeout) is the longest silence the client tolerates between
/// reply frames and is not sent, and its [`deadline`](StreamCall::deadline) bounds the whole call
/// and is sent as `deadline-ms`. A stream has no deadline unless the call sets one, since a stream
/// may run indefinitely, and no idle timeout unless the call or the client's
/// [`idle_timeout`](Self::idle_timeout) sets one.
///
/// `deadline-ms` is what is left of the call's bound when its frame is handed to a connection
/// already made: a slow connect gives the server no longer than the caller waits, and a bound that
/// passes while the client connects fails the call `Timeout` with nothing sent. A `Timeout` once
/// the frame has been handed to the link leaves the call's outcome unknown, on every shape: the
/// server may have run the request, and an emit's event may have been published on a broker that
/// confirms publishes.
///
/// A bound too long for the runtime's clock to reach, `Duration::MAX` among them, is no bound, as
/// `Bound::Unbounded` is: no timer is armed for it and no `deadline-ms` is sent.
#[derive(Clone)]
pub struct RpcClient {
    pub(crate) inner: Arc<ClientInner>,
    /// Every call's timeout unless the call sets its own; `None` for `Bound::Unbounded`. Held by
    /// the handle, so [`timeout`](Self::timeout) on one clone leaves the others' as they were.
    timeout: Option<Duration>,
    /// Every stream's idle timeout unless the call sets its own; `None` for `Bound::Unbounded`
    /// and `Bound::Default`. Held by the handle, as `timeout` is.
    idle: Option<Duration>,
}

type Connect = Box<dyn Fn() -> BoxFuture<'static, Result<Outbound, BoxError>> + Send + Sync>;

type Close = Box<dyn Fn() -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>;

pub(crate) struct ClientInner {
    link: &'static str,
    connect: Connect,
    /// The link's `close`, for a client that owns its link, one built by [`RpcClient::new`]; a
    /// module's client leaves it to the module's destroy hook.
    close: Option<Close>,
    codec: Codec,
    runtime: Arc<dyn Runtime>,
    /// Held across `connect`, so concurrent first calls share one connection.
    conn: async_lock::Mutex<Option<Arc<Conn>>>,
    next_id: AtomicU64,
}

/// One connection of the link's client side.
struct Conn {
    send: Box<dyn Fn(Pattern, Frame, Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> + Send + Sync>,
    pending: Mutex<HashMap<u64, Waiting>>,
    /// False once the reply lane ended or the server sent `goaway`: no new call goes over it.
    open: AtomicBool,
    /// The client's runtime, which a dropped call's `cancel` is spawned on.
    runtime: Arc<dyn Runtime>,
}

/// Where the reply frames of one call go, and a streamed request's pump reports its failure.
type Waiting = UnboundedSender<Result<Frame, RpcError>>;

impl Conn {
    fn pending(&self) -> MutexGuard<'_, HashMap<u64, Waiting>> {
        self.pending.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn send(&self, pattern: &Pattern, frame: Frame, reply_to: Option<ReplyTo>) -> BoxFuture<'static, Result<(), BoxError>> {
        (self.send)(pattern.clone(), frame, reply_to)
    }

    /// Registers call `id` for its replies: `false`, registering nothing, once the reply lane has
    /// ended or a `goaway` arrived. Checked under the lock [`ended`](Self::ended) clears under, so
    /// a call registered here is answered by the lane or failed by its end, never left waiting on
    /// a lane that has already gone.
    fn register(&self, id: u64, waiting: Waiting) -> bool {
        let mut pending = self.pending();
        if !self.open.load(Ordering::Acquire) {
            return false;
        }
        pending.insert(id, waiting);
        true
    }

    /// The reply lane's end: no new call goes over the connection, and every call waiting on it
    /// fails `Unavailable`.
    fn ended(&self) {
        let mut pending = self.pending();
        self.open.store(false, Ordering::Release);
        pending.clear();
    }
}

impl RpcClient {
    /// A client of `link` outside an app, its tasks spawned on `runtime`, which also times its
    /// calls: five seconds for each `request`, `send_stream` and `emit` unless
    /// [`timeout`](Self::timeout) or the call sets another.
    ///
    /// ```
    /// # use std::sync::Arc;
    /// # use std::time::Duration;
    /// # use ulo::Bound;
    /// # use ulo_rpc::RpcClient;
    /// # fn main() -> Result<(), Box<dyn std::error::Error>> {
    /// # let tokio = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    /// # let _entered = tokio.enter();
    /// let billing = RpcClient::new(ulo_rpc_tcp::Tcp::new("10.0.0.5:7000"), Arc::new(ulo_runtime_tokio::Tokio::current()))?
    ///     .timeout(Bound::After(Duration::from_secs(2)))?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// A link that can never connect is refused here as [`UnusableLink`], where the mistake is
    /// made, rather than failing each call `Unavailable`: [`Link::usable`] decides, a tokio-based
    /// link built outside a runtime and given none, or a TCP link given a host name, since it takes
    /// IP addresses only. Nothing else about the link is checked before its first call.
    ///
    /// The client owns the link. Once its last clone and the last call it made are dropped, it
    /// releases its connection and spawns the link's `close` on `runtime`. A runtime that drops
    /// that task before it finishes, as one that has shut down does, leaves the link's close
    /// undone, logged at `warn`.
    pub fn new<L: Link>(link: L, runtime: Arc<dyn Runtime>) -> Result<Self, UnusableLink> {
        if let Err(source) = link.usable() {
            return Err(UnusableLink { site: "RpcClient::new", link: L::NAME, source });
        }
        let link = Arc::new(link);
        let closing = Arc::clone(&link);
        let close: Close = Box::new(move || {
            let link = Arc::clone(&closing);
            Box::pin(async move { link.close().await })
        });
        Ok(RpcClient::build(link, Bound::Default, Bound::Default, runtime, Some(close)))
    }

    /// `RpcClientModule`'s client, whose link the module's destroy hook closes.
    pub(crate) fn of_module<L: Link>(link: Arc<L>, timeout: Bound, idle: Bound, runtime: Arc<dyn Runtime>) -> Self {
        RpcClient::build(link, timeout, idle, runtime, None)
    }

    fn build<L: Link>(link: Arc<L>, timeout: Bound, idle: Bound, runtime: Arc<dyn Runtime>, close: Option<Close>) -> Self {
        let codec = Codec::of(&link.capabilities());
        let handed = Arc::clone(&runtime);
        let connect: Connect = Box::new(move || {
            let (link, runtime) = (Arc::clone(&link), Arc::clone(&handed));
            Box::pin(async move { link.connect(&runtime).await })
        });
        RpcClient {
            inner: Arc::new(ClientInner {
                link: L::NAME,
                connect,
                close,
                codec,
                runtime,
                conn: async_lock::Mutex::new(None),
                next_id: AtomicU64::new(1),
            }),
            timeout: timeout_of(timeout),
            idle: idle_of(idle),
        }
    }

    /// The timeout of every `request` and `send_stream` that sets none of its own, the whole
    /// call's bound and its `deadline-ms`, and the bound on every `emit`'s publish, as
    /// `RpcClientModule::timeout` sets it for a module's client: five seconds at `Bound::Default`,
    /// `Bound::Unbounded` for none. A `stream` or `duplex` call is bounded only by its own
    /// [`idle_timeout`](StreamCall::idle_timeout) and [`deadline`](StreamCall::deadline), so this
    /// leaves it unbounded. A call made [`within`](Call::within) an execution is sent the shorter
    /// of the execution's remaining time and this timeout, so a caller with more time left than
    /// the timeout reaches the callee with the timeout. This handle and the clones made from it
    /// after carry it; clones made
    /// before keep theirs, sharing the connection all the same.
    ///
    /// `Bound::After(Duration::ZERO)`, which would time out every call, is refused as
    /// [`ZeroTimeout`], and the handle with it, so `RpcClient::new(link, runtime)?.timeout(b)?`
    /// fails on the line that wrote the zero. `RpcClientModule` refuses it when the app wires.
    pub fn timeout(mut self, timeout: Bound) -> Result<Self, ZeroTimeout> {
        if timeout == Bound::After(Duration::ZERO) {
            return Err(ZeroTimeout::of_bound("RpcClient::timeout", self.inner.link, "every call"));
        }
        self.timeout = timeout_of(timeout);
        Ok(self)
    }

    /// The idle timeout of every `stream` and `duplex` call that leaves its own at
    /// `Bound::Default`, as `RpcClientModule::idle_timeout` sets it for a module's client: the
    /// longest silence the client tolerates between reply frames, as [`StreamCall::idle_timeout`]
    /// describes, and not sent. None at `Bound::Default`, as at `Bound::Unbounded`, so a quiet
    /// stream, a subscription yielding now and then, runs until it ends. A lost connection fails a
    /// waiting stream `Unavailable` through the reply lane whatever the idle timeout, so the idle
    /// timeout catches only a server that is still connected and has gone silent. This handle and
    /// the clones made from it after carry it, as [`timeout`](Self::timeout)'s do.
    ///
    /// `Bound::After(Duration::ZERO)`, which would end every stream before its first frame, is
    /// refused as [`ZeroTimeout`].
    pub fn idle_timeout(mut self, idle: Bound) -> Result<Self, ZeroTimeout> {
        if idle == Bound::After(Duration::ZERO) {
            return Err(ZeroTimeout::of_bound("RpcClient::idle_timeout", self.inner.link, "every stream"));
        }
        self.idle = idle_of(idle);
        Ok(self)
    }

    /// A request answered by one reply.
    pub fn request<Req, Res>(&self, pattern: &str, req: &Req) -> Call<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned + Send + 'static,
    {
        let payload = encode(self.inner.codec, req);
        Call { _res: PhantomData, spec: Spec::new(self, pattern, Request::One(payload)) }
    }

    /// An event, answered by nothing.
    pub fn emit<T: Serialize + ?Sized>(&self, pattern: &str, data: &T) -> Emit {
        let payload = encode(self.inner.codec, data);
        Emit { spec: Spec::new(self, pattern, Request::One(payload)) }
    }

    /// A request answered by a stream of replies.
    pub fn stream<Req, Res>(&self, pattern: &str, req: &Req) -> StreamCall<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned + Send + 'static,
    {
        let payload = encode(self.inner.codec, req);
        StreamCall::new(Spec::new(self, pattern, Request::One(payload)))
    }

    /// A streamed request answered by one reply.
    pub fn send_stream<S, Res>(&self, pattern: &str, items: S) -> Call<Res>
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
        Res: DeserializeOwned + Send + 'static,
    {
        Call { _res: PhantomData, spec: Spec::new(self, pattern, self.items(items)) }
    }

    /// A streamed request answered by a stream of replies.
    pub fn duplex<S, Res>(&self, pattern: &str, items: S) -> StreamCall<Res>
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
        Res: DeserializeOwned + Send + 'static,
    {
        StreamCall::new(Spec::new(self, pattern, self.items(items)))
    }

    fn items<S>(&self, items: S) -> Request
    where
        S: Stream + Send + 'static,
        S::Item: Serialize,
    {
        let codec = self.inner.codec;
        Request::Stream(Box::pin(items.map(move |item| encode(codec, &item))))
    }
}

/// `value` as a payload. A JSON link carries raw bytes only as an array of numbers, so a payload
/// that writes them is refused here, before any I/O.
fn encode<T: Serialize + ?Sized>(codec: Codec, value: &T) -> Result<Data, RpcError> {
    match codec.encode_checked(value) {
        Ok((_, true)) if !codec.binary() => Err(RpcError::new(
            ErrorKind::BadRequest,
            "a binary payload cannot travel on a JSON link; set `.codec(Codec::Cbor)` on the link",
        )
        .with_reason("binary_unsupported")),
        Ok((data, _)) => Ok(data),
        Err(err) => Err(RpcError::new(ErrorKind::BadRequest, format!("the payload could not be encoded: {err}"))),
    }
}

/// What every builder carries.
pub(crate) struct Spec {
    client: RpcClient,
    pattern: Pattern,
    request: Request,
    headers: CallHeaders,
    /// The call's own timeout, in place of the client's.
    timeout: Option<Duration>,
    within: Option<ExecutionRef>,
    /// Where the call's whole bound ends, set by [`limit`](Self::limit): what is left of it when the
    /// frame is written is the call's `deadline-ms`, a call it ends before then is not sent, and the
    /// call's own timer fires there.
    ends: Option<Instant>,
    /// Whether the call carries `deadline-ms`: every shape but an event.
    stamps: bool,
}

/// `deadline-ms` for `remaining`: whole milliseconds, a positive remainder under one millisecond
/// written as `1`, and a remainder longer than the server reads, `u64::MAX` milliseconds, about
/// 584 million years, written as that.
fn deadline_ms(remaining: Duration) -> String {
    u64::try_from(remaining.as_millis()).unwrap_or(u64::MAX).max(1).to_string()
}

/// What bounds a call as a whole, and whether the bound is sent as `deadline-ms`.
#[derive(Clone, Copy)]
enum Whole {
    /// A call answered by one reply: its own timeout or the client's, sent. The client abandons
    /// the call there, so the server stops there too.
    Reply,
    /// An event: the client's timeout, covering the publish. Nothing is sent, the calling
    /// execution's remaining time included, since the handler runs with no caller waiting.
    Publish,
    /// A stream: its own deadline, sent, or none.
    Stream(Option<Duration>),
}

enum Request {
    One(Result<Data, RpcError>),
    Stream(BoxStream<'static, Result<Data, RpcError>>),
}

impl Spec {
    fn new(client: &RpcClient, pattern: &str, request: Request) -> Self {
        Spec {
            client: client.clone(),
            pattern: Pattern::from(pattern),
            request,
            headers: CallHeaders::new(),
            timeout: None,
            within: None,
            ends: None,
            stamps: false,
        }
    }

    /// The instant the whole call's wait ends, `whole`'s bound or the calling execution's
    /// remaining time when that is shorter, counted from now. Refuses a call whose execution's
    /// deadline has passed, before any I/O.
    ///
    /// Every shape but an event carries what is left of that wait as `deadline-ms`, the deadline
    /// the server enforces, measured when the frame is written: [`written`] stamps it. An event's
    /// handler has no caller waiting on it, often runs after its emitter has ended, and on a
    /// broker that holds it would count the remainder from delivery, so it is given none.
    fn limit(&mut self, whole: Whole) -> Result<Option<Instant>, RpcError> {
        let inner = &self.client.inner;
        let now = inner.runtime.now();
        let own = match whole {
            Whole::Reply | Whole::Publish => self.timeout.or(self.client.timeout),
            Whole::Stream(deadline) => deadline,
        };
        // A bound whose end an `Instant` cannot hold is none, as `Bound::Unbounded` is: `now +
        // own` would panic, and a timer handed it computes the same sum.
        let own = own.filter(|own| now.checked_add(*own).is_some());
        let remaining = match self.within.as_ref().and_then(ExecutionRef::deadline) {
            Some(deadline) => {
                let remaining = deadline.saturating_duration_since(now);
                if remaining.is_zero() {
                    return Err(RpcError::new(ErrorKind::Timeout, "the calling execution's deadline has passed"));
                }
                Some(remaining)
            }
            None => None,
        };
        let after = match (own, remaining) {
            (Some(own), Some(remaining)) => Some(own.min(remaining)),
            (own, remaining) => own.or(remaining),
        };
        let ends = after.and_then(|after| now.checked_add(after));
        self.ends = ends;
        self.stamps = !matches!(whole, Whole::Publish);
        Ok(ends)
    }

    /// Refuses a call whose caller set a header the client writes itself, `BadRequest` with the
    /// reason `"reserved_header"`:
    ///
    /// - `deadline-ms`, readable or not: the client writes that header from the call's bound, so a
    ///   caller's value either contradicts the bound or, where the call has none, sets one the
    ///   client does not enforce. The first such value is quoted as the server's `bad_deadline`
    ///   quotes one; `instead` says what to write in its place.
    /// - `traceparent`, exactly on a call made `.within` an execution that carries a trace
    ///   context, forwarded or its own: the client writes it from that context (I8).
    /// - `tracestate` on such a call when the context is `TraceOrigin::Received`: the client
    ///   forwards the list as it arrived, which W3C Trace Context §3.4 forbids modifying, and only a
    ///   participant, an execution whose context a `TraceSource` set, may add an entry. On an
    ///   `Originated` context the caller's entries are merged instead, when the frame is written.
    ///
    /// A call with no `.within`, or within an execution carrying no context, a `TraceMode::Off`
    /// app's among them, keeps both as ordinary headers. The trace headers' names are matched
    /// without regard to case, as W3C Trace Context matches them.
    fn reserved(&self, instead: &str) -> Result<(), RpcError> {
        if let Some(value) = self.headers.get(DEADLINE_MS) {
            let message = format!(
                "the call set `{DEADLINE_MS}` to {} with `.header`, and the client reserves that header, writing it from the call's bound; {instead}",
                quote(value)
            );
            return Err(RpcError::new(ErrorKind::BadRequest, message).with_reason("reserved_header"));
        }
        let Some(context) = self.within.as_ref().and_then(ExecutionRef::trace) else { return Ok(()) };
        if self.header_named(TRACEPARENT) {
            let message = format!(
                "the call set `{TRACEPARENT}` with `.header`, and the client reserves that header on a call made `.within` an execution carrying a trace context, writing it from that context"
            );
            return Err(RpcError::new(ErrorKind::BadRequest, message).with_reason("reserved_header"));
        }
        if context.origin() == TraceOrigin::Received && self.header_named(TRACESTATE) {
            let message = format!(
                "the call set `{TRACESTATE}` with `.header` within an execution forwarding the context it received, which the client passes on unchanged; adding a vendor entry needs an exporter, a `TraceSource` such as `ulo-otel`'s, to originate the execution's context"
            );
            return Err(RpcError::new(ErrorKind::BadRequest, message).with_reason("reserved_header"));
        }
        Ok(())
    }

    fn header_named(&self, name: &str) -> bool {
        self.headers.iter().any(|(key, _)| key.eq_ignore_ascii_case(name))
    }

    /// Connects, registers the call, and sends its opening frame; a streamed request's items follow
    /// from a task of their own.
    async fn open(self) -> Result<Exchange, RpcError> {
        let Spec { client, pattern, request, mut headers, within, ends, stamps, .. } = self;
        let inner = &client.inner;
        let request = match request {
            Request::One(payload) => Request::One(Ok(payload?)),
            stream => stream,
        };
        let (conn, id, waiting, replies) = inner.register().await?;
        let mut pending = Pending { conn: Arc::clone(&conn), id, pattern: pattern.clone(), settled: false };
        if let Err(err) = written(&mut headers, within.as_ref(), ends, stamps, inner.runtime.now()) {
            // Nothing reached a server that a `cancel` would stop.
            pending.settled = true;
            return Err(err);
        }
        let text = pattern.as_str().to_owned();
        let sent = match request {
            Request::One(payload) => {
                let data = payload?;
                conn.send(&pattern, Frame::Req { id, pattern: text, headers, data }, Some(ReplyTo { id })).await
            }
            Request::Stream(items) => {
                let opened = conn.send(&pattern, Frame::Open { id, pattern: text, headers }, Some(ReplyTo { id })).await;
                if opened.is_ok() {
                    // Detached: the pump stops once the call has ended.
                    drop(inner.runtime.spawn(Box::pin(pump(Arc::clone(&conn), pattern, id, items, waiting))));
                }
                opened
            }
        };
        if let Err(err) = sent {
            // Nothing reached a server that a `cancel` would stop.
            pending.settled = true;
            return Err(send_error(err));
        }
        let timer: Arc<dyn Timer> = inner.runtime.clone();
        Ok(Exchange { replies, pending, codec: inner.codec, timer, within, _client: client.clone() })
    }
}

/// What a call's frame carries of its bound and its trace context when it is written at `now`,
/// its connection in hand: the calling execution's context as `traceparent` and `tracestate`, on
/// every shape, events included, whether or not the call has a bound (I6); and `deadline-ms`, what
/// is left of the bound ending at `ends`, when `stamps`. A bound that ended while the client
/// connected fails the call `Timeout` and its frame is never sent: written then, it would reach a
/// server that runs the call while its caller is told it timed out.
///
/// Measured with the connection in hand, a call on a new connection gives its server none of the
/// connect's length beyond the caller's wait.
fn written(headers: &mut CallHeaders, within: Option<&ExecutionRef>, ends: Option<Instant>, stamps: bool, now: Instant) -> Result<(), RpcError> {
    // A caller's `traceparent`, and its `tracestate` beside a received context, were refused by
    // `reserved`, so what this writes is the only copy.
    if let Some(context) = within.and_then(ExecutionRef::trace) {
        trace::inject(headers, context);
    }
    let Some(ends) = ends else { return Ok(()) };
    let left = ends.saturating_duration_since(now);
    if left.is_zero() {
        return Err(RpcError::new(ErrorKind::Timeout, "the call's deadline passed while its client connected, so nothing was sent"));
    }
    // A `deadline-ms` the caller set was refused by `reserved`, so the stamp is the only one.
    if stamps {
        headers.insert(DEADLINE_MS, deadline_ms(left));
    }
    Ok(())
}

impl ClientInner {
    /// A connection with a new call registered on it, and where its replies arrive. A connection
    /// whose reply lane ends between [`connection`](Self::connection) handing it over and the
    /// registration is replaced once by a new one; a second that ends as fast fails the call
    /// `Unavailable`.
    async fn register(&self) -> Result<(Arc<Conn>, u64, Waiting, UnboundedReceiver<Result<Frame, RpcError>>), RpcError> {
        for _ in 0..2 {
            let conn = self.connection().await?;
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (waiting, replies) = mpsc::unbounded();
            if conn.register(id, waiting.clone()) {
                return Ok((conn, id, waiting, replies));
            }
        }
        Err(lost())
    }

    async fn connection(&self) -> Result<Arc<Conn>, RpcError> {
        let mut slot = self.conn.lock().await;
        if let Some(conn) = slot.as_ref().filter(|conn| conn.open.load(Ordering::Acquire)) {
            return Ok(Arc::clone(conn));
        }
        let Outbound { send, replies } = (self.connect)().await.map_err(|err| connect_error(self.link, err))?;
        let conn = Arc::new(Conn {
            send,
            pending: Mutex::new(HashMap::new()),
            open: AtomicBool::new(true),
            runtime: Arc::clone(&self.runtime),
        });
        // Detached: it ends with the reply lane, or at the next frame once the connection is gone.
        drop(self.runtime.spawn(Box::pin(route_replies(Arc::downgrade(&conn), replies))));
        *slot = Some(Arc::clone(&conn));
        Ok(conn)
    }
}

/// The reply lane of one connection, routed by id. It holds the connection weakly, so a client
/// dropped with no call in flight releases it at the next frame.
async fn route_replies(conn: Weak<Conn>, mut replies: BoxStream<'static, Frame>) {
    while let Some(frame) = replies.next().await {
        let Some(conn) = conn.upgrade() else { return };
        if matches!(frame, Frame::Goaway) {
            conn.open.store(false, Ordering::Release);
            continue;
        }
        let Some(id) = frame.id() else { continue };
        let ends = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
        let waiting = {
            let mut pending = conn.pending();
            if ends { pending.remove(&id) } else { pending.get(&id).cloned() }
        };
        if let Some(waiting) = waiting {
            let _ = waiting.unbounded_send(Ok(frame));
        }
    }
    if let Some(conn) = conn.upgrade() {
        conn.ended();
    }
}

/// A streamed request's items as `in` frames, then `in_end`. A failure goes to the waiting call,
/// which fails with it; the pump stops once the call has ended.
async fn pump(conn: Arc<Conn>, pattern: Pattern, id: u64, mut items: BoxStream<'static, Result<Data, RpcError>>, waiting: Waiting) {
    while let Some(item) = items.next().await {
        if waiting.is_closed() {
            return;
        }
        let data = match item {
            Ok(data) => data,
            Err(err) => {
                let _ = waiting.unbounded_send(Err(err));
                return;
            }
        };
        if let Err(err) = conn.send(&pattern, Frame::In { id, data }, None).await {
            let _ = waiting.unbounded_send(Err(send_error(err)));
            return;
        }
    }
    if let Err(err) = conn.send(&pattern, Frame::InEnd { id }, None).await {
        let _ = waiting.unbounded_send(Err(send_error(err)));
    }
}

/// A call registered on its connection. Dropped before its reply settled it, a timeout, a cancel
/// or the caller's own drop, it sends `cancel`.
struct Pending {
    conn: Arc<Conn>,
    id: u64,
    pattern: Pattern,
    settled: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        self.conn.pending().remove(&self.id);
        if self.settled {
            return;
        }
        let cancel = self.conn.send(&self.pattern, Frame::Cancel { id: self.id }, None);
        drop(self.conn.runtime.spawn(Box::pin(async move {
            let _ = cancel.await;
        })));
    }
}

/// A client that owns its link closes it from a task on its runtime, a drop being unable to
/// await. Its connection is released as its fields drop, whatever the runtime does with the task.
impl Drop for ClientInner {
    fn drop(&mut self) {
        let Some(close) = &self.close else { return };
        let closing = close();
        let link = self.link;
        let unfinished = Unfinished { link, finished: false };
        drop(self.runtime.spawn(Box::pin(async move {
            let closed = closing.await;
            unfinished.finish();
            if let Err(error) = closed {
                tracing::warn!(%error, link, "the RPC client's link did not close cleanly");
            }
        })));
    }
}

/// The link's close spawned by a dropped client, reported at `warn` if its task is dropped
/// before the close finished: a runtime that has shut down drops a task it is handed unrun.
struct Unfinished {
    link: &'static str,
    finished: bool,
}

impl Unfinished {
    fn finish(mut self) {
        self.finished = true;
    }
}

impl Drop for Unfinished {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                link = self.link,
                "the RPC client was dropped and its runtime dropped the link's close before it finished; its connection was released"
            );
        }
    }
}

/// One call's reply frames.
struct Exchange {
    replies: UnboundedReceiver<Result<Frame, RpcError>>,
    pending: Pending,
    codec: Codec,
    timer: Arc<dyn Timer>,
    within: Option<ExecutionRef>,
    /// Keeps a client that owns its link from closing it while this call is under way.
    _client: RpcClient,
}

impl Exchange {
    async fn single<Res: DeserializeOwned>(mut self) -> Result<Res, RpcError> {
        match self.replies.next().await {
            Some(Ok(Frame::Res { data, .. })) => {
                self.pending.settled = true;
                decode(self.codec, &data)
            }
            Some(Ok(Frame::Err { error, .. })) => {
                self.pending.settled = true;
                Err(RpcError::from_body(error))
            }
            Some(Ok(other)) => Err(unexpected(&other)),
            Some(Err(err)) => Err(err),
            None => Err(lost()),
        }
    }
}

fn decode<Res: DeserializeOwned>(codec: Codec, data: &Data) -> Result<Res, RpcError> {
    codec.decode(data).map_err(|err| RpcError::new(ErrorKind::Internal, format!("the reply could not be decoded: {err}")))
}

fn unexpected(frame: &Frame) -> RpcError {
    RpcError::new(ErrorKind::Internal, format!("the server answered with an unexpected `{}` frame", frame.kind()))
}

fn lost() -> RpcError {
    RpcError::new(ErrorKind::Unavailable, "the link closed before the reply arrived")
}

/// A link's failure to connect, mapped one way on every link: `Unavailable`, which the next call
/// may not meet, unless the link answered it [`Misconfigured`], which the next call meets too,
/// save a `tls://` NATS link's roots wait that outlasted its bound, which ends once the load has.
/// A connect the link's runtime refused, having shut down, a wait of the connect's or the connect
/// itself, answers a [`RuntimeShutDown`](ulo::RuntimeShutDown), which keeps its reason. A connect
/// that panicked keeps the link's error as the call's source, so [`ulo::is_panic`] finds the panic
/// on the call's error.
fn connect_error(link: &str, err: BoxError) -> RpcError {
    if ulo::is_panic(&err) {
        let mut error = RpcError::new(ErrorKind::Unavailable, format!("the {link} link could not connect: {err}"));
        error.source = Some(err);
        error
    } else if err.is::<Misconfigured>() {
        RpcError::new(ErrorKind::Internal, format!("the {link} link cannot connect: {err}")).with_reason("misconfigured_link")
    } else if err.is::<ulo::RuntimeShutDown>() {
        RpcError::new(ErrorKind::Unavailable, format!("the {link} link could not connect: {err}")).with_reason(ulo::RUNTIME_SHUT_DOWN)
    } else if err.is::<ConnectTimedOut>() {
        RpcError::new(ErrorKind::Unavailable, format!("the {link} link could not connect: {err}")).with_reason("connect_timed_out")
    } else {
        RpcError::new(ErrorKind::Unavailable, format!("the {link} link could not connect: {err}"))
    }
}

/// A link's refusal of a frame, mapped one way on every link.
fn send_error(err: BoxError) -> RpcError {
    if err.is::<NoDestination>() {
        RpcError::new(ErrorKind::Unavailable, err.to_string()).with_reason("no_destination")
    } else if err.is::<FrameTooLarge>() {
        RpcError::new(ErrorKind::BadRequest, err.to_string()).with_reason("payload_too_large")
    } else if err.is::<FrameUnencodable>() {
        RpcError::new(ErrorKind::Internal, err.to_string())
    } else {
        RpcError::new(ErrorKind::Unavailable, format!("the link refused the frame: {err}"))
    }
}

/// One wait's bound, the instant it ends on the client's runtime's clock, and what the caller is
/// told when it passes.
#[derive(Clone, Copy)]
struct Limit {
    ends: Instant,
    passed: &'static str,
}

impl Limit {
    /// A call's whole bound, its timeout or its execution's remaining time, ending where
    /// [`Spec::limit`] put it.
    fn call(ends: Option<Instant>) -> Option<Limit> {
        ends.map(|ends| Limit { ends, passed: "the call timed out" })
    }

    /// A stream's wait, from `now`, for its next reply frame: its idle timeout, or its deadline at
    /// `ends` when that comes first. An idle timeout whose end an `Instant` cannot hold is none, as
    /// `Bound::Unbounded` is.
    fn frame(idle: Option<Duration>, ends: Option<Instant>, now: Instant) -> Option<Limit> {
        const IDLE: &str = "no reply frame arrived within the stream's idle timeout";
        const DEADLINE: &str = "the stream's deadline passed";
        let idle = idle.and_then(|idle| now.checked_add(idle));
        match (idle, ends) {
            (Some(idle), Some(ends)) if ends < idle => Some(Limit { ends, passed: DEADLINE }),
            (Some(idle), _) => Some(Limit { ends: idle, passed: IDLE }),
            (None, Some(ends)) => Some(Limit { ends, passed: DEADLINE }),
            (None, None) => None,
        }
    }
}

/// `work` bounded by `limit` on the client's runtime and by the calling execution's cancellation.
/// An execution already cancelled fails it before `work` is polled at all, so a call's frame is
/// never sent for it. The bound is a [`ulo::timeout_at`], asked of the clock for what is left of it
/// before `work` is first polled, so work that blocks its thread there cannot push the wait past
/// the bound's end. A runtime that refuses it, having shut down, fails the call with
/// [`runtime_shut_down`] rather than `Timeout`, under the core's ties (DESIGN §3.9).
async fn bounded<T>(
    timer: &dyn Timer,
    limit: Option<Limit>,
    within: Option<&ExecutionRef>,
    work: impl Future<Output = Result<T, RpcError>>,
) -> Result<T, RpcError> {
    // The `select!` below picks at random among the arms ready at once, so a cancellation that
    // had already fired would race the first poll of `work`, which can send the call's frame.
    if let Some(exec) = within.filter(|exec| exec.is_cancelled()) {
        return Err(cancellation(exec.cancel_reason()));
    }
    let timed = async {
        match limit {
            Some(limit) => match ulo::timeout_at(timer, limit.ends, work).await {
                Ok(out) => out,
                Err(ulo::Cut::Expired) => Err(RpcError::new(ErrorKind::Timeout, limit.passed)),
                Err(ulo::Cut::Refused(refused)) => Err(runtime_shut_down(refused)),
            },
            None => work.await,
        }
    };
    let cancelled = async {
        match within {
            Some(exec) => {
                exec.cancelled().await;
                exec.cancel_reason()
            }
            None => std::future::pending().await,
        }
    };
    // Unbiased: when both are ready, one is picked at random.
    futures_util::select! {
        out = timed.fuse() => out,
        reason = cancelled.fuse() => Err(cancellation(reason)),
    }
}

/// What a call fails with when the runtime timing it has shut down: `Unavailable` with the reason
/// [`RUNTIME_SHUT_DOWN`](ulo::RUNTIME_SHUT_DOWN) and the runtime's [`Refused`](ulo::Refused) as
/// its source.
fn runtime_shut_down(refused: ulo::Refused) -> RpcError {
    RpcError::new(ErrorKind::Unavailable, "the runtime timing this call has shut down").with_reason(ulo::RUNTIME_SHUT_DOWN).with_source(refused)
}

/// What a call fails with when its calling execution is cancelled for `reason`.
fn cancellation(reason: Option<CancelReason>) -> RpcError {
    match reason {
        Some(CancelReason::Deadline) => RpcError::new(ErrorKind::Timeout, "the calling execution's deadline passed"),
        _ => RpcError::new(ErrorKind::Unavailable, "the calling execution was cancelled"),
    }
}

/// A call answered by one reply, sent when awaited.
#[must_use = "a call is sent when awaited"]
pub struct Call<Res> {
    pub(crate) _res: PhantomData<fn() -> Res>,
    pub(crate) spec: Spec,
}

impl<Res> Call<Res> {
    /// A header on the call, `h` or the broker's own headers. `deadline-ms` is reserved: the
    /// client writes it from [`timeout`](Self::timeout) or [`within`](Self::within), and a call
    /// given it here fails when it is awaited, before it connects, `BadRequest` with the reason
    /// `"reserved_header"`. On a call made `.within` an execution carrying a trace context,
    /// `traceparent` is reserved the same way, and `tracestate` too when that context is one the
    /// execution received; within one a `TraceSource` originated, a `tracestate` set here is
    /// merged into the execution's list, its entries at the left (integrations §6.3).
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

    /// This call's timeout in place of the client's, and the deadline the server is given in
    /// `deadline-ms` unless a shorter one applies.
    ///
    /// `Duration::ZERO`, which would time out the call at once, fails the call when it is awaited,
    /// before it connects, `BadRequest` with the reason `"zero_timeout"` and a [`ZeroTimeout`] as
    /// its source, so no request reaches the server. A call cannot turn the client's timeout off:
    /// one with none is made on a client given [`RpcClient::timeout`]`(Bound::Unbounded)`.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.spec.timeout = Some(timeout);
        self
    }

    /// Forwards `exec`'s remaining deadline as `deadline-ms` and its trace context as
    /// `traceparent` and `tracestate`, and cancels the call when `exec` is cancelled. A call made
    /// once `exec`'s deadline has passed, or once it is cancelled, fails when it is awaited, before
    /// it connects, `Timeout` or `Unavailable`, so nothing reaches the server.
    ///
    /// The deadline sent is the shorter of `exec`'s remaining time and the call's own bound, its
    /// [`timeout`](Self::timeout) or the client's, five seconds at `Bound::Default`. A caller with
    /// thirty seconds left reaches the callee with about five on a default client, because the
    /// client stops waiting then and the callee is not to outlive its caller.
    ///
    /// ```
    /// # use std::time::Duration;
    /// # use ulo::ExecutionRef;
    /// # use ulo_rpc::{RpcClient, RpcError};
    /// async fn quote(pricing: &RpcClient, exec: &ExecutionRef) -> Result<u64, RpcError> {
    ///     // With 30 s left on `exec`, the callee is given about 5 s (the client's default
    ///     // timeout); with 2 s left, about 2 s.
    ///     pricing.request::<_, u64>("price.quote", "sku-1").within(exec).await
    /// }
    ///
    /// async fn quote_patiently(pricing: &RpcClient, exec: &ExecutionRef) -> Result<u64, RpcError> {
    ///     // A longer bound on the call lifts the 5 s cap; `exec`'s remaining time still applies.
    ///     pricing.request::<_, u64>("price.quote", "sku-1").timeout(Duration::from_secs(20)).within(exec).await
    /// }
    /// ```
    pub fn within(mut self, exec: &ExecutionRef) -> Self {
        self.spec.within = Some(exec.clone());
        self
    }
}

impl<Res: DeserializeOwned + Send + 'static> IntoFuture for Call<Res> {
    type Output = Result<Res, RpcError>;
    type IntoFuture = BoxFuture<'static, Result<Res, RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        let mut spec = self.spec;
        Box::pin(async move {
            spec.reserved("bound the call with `.timeout`, or forward an execution's deadline with `.within`")?;
            // The builder answers `Self`, so the await is the first point a zero can fail, before
            // the call connects: timed out at once, its `req` would reach the server or not by
            // the race in `bounded`.
            if spec.timeout == Some(Duration::ZERO) {
                return Err(ZeroTimeout::of_duration(
                    "Call::timeout",
                    spec.client.inner.link,
                    "the call",
                    "leave `.timeout` unset for the client's timeout, or make the call on a client whose timeout is `Bound::Unbounded`",
                )
                .refusal());
            }
            let limit = Limit::call(spec.limit(Whole::Reply)?);
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            bounded(&*runtime, limit, within.as_ref(), async move { spec.open().await?.single::<Res>().await }).await
        })
    }
}

/// An event, published when awaited.
#[must_use = "an event is published when awaited"]
pub struct Emit {
    pub(crate) spec: Spec,
}

impl Emit {
    /// A header on the event. `deadline-ms` is reserved, as on [`Call::header`]: an event given it
    /// fails when it is awaited, before it connects, `BadRequest` with the reason
    /// `"reserved_header"`. `traceparent` and `tracestate` follow [`Call::header`]'s rule.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

    /// Cancels the publish when `exec` is cancelled, and bounds it by `exec`'s remaining time, up
    /// to the send: an event emitted once `exec` is cancelled or its deadline has passed fails when
    /// it is awaited, before it connects, and one sent before then is handled whatever becomes of
    /// `exec`. The event carries none of `exec`'s deadline: nobody waits on it, so its handler runs
    /// under no deadline. It carries `exec`'s trace context, so the handler's span is a consumer of
    /// the emitter's.
    ///
    /// A deadline that passes while the client connects fails the emit `Timeout` with nothing
    /// sent. A `Timeout` once the publish has started leaves the event's fate unknown: on a broker
    /// that confirms publishes, the event may have been published.
    pub fn within(mut self, exec: &ExecutionRef) -> Self {
        self.spec.within = Some(exec.clone());
        self
    }
}

impl IntoFuture for Emit {
    type Output = Result<(), RpcError>;
    type IntoFuture = BoxFuture<'static, Result<(), RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        let mut spec = self.spec;
        Box::pin(async move {
            spec.reserved("an event is sent with no deadline")?;
            let limit = Limit::call(spec.limit(Whole::Publish)?);
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            bounded(&*runtime, limit, within.as_ref(), async move {
                let Spec { client, pattern, request, mut headers, within, ends, stamps, .. } = spec;
                let data = match request {
                    Request::One(payload) => payload?,
                    Request::Stream(_) => return Err(RpcError::new(ErrorKind::BadRequest, "an event carries one payload")),
                };
                let conn = client.inner.connection().await?;
                written(&mut headers, within.as_ref(), ends, stamps, client.inner.runtime.now())?;
                let frame = Frame::Evt { pattern: pattern.as_str().to_owned(), headers, data };
                conn.send(&pattern, frame, None).await.map_err(send_error)
            })
            .await
        })
    }
}

/// A call answered by a stream, opened when awaited: a [`stream`](RpcClient::stream) or a
/// [`duplex`](RpcClient::duplex) call. It has no `timeout`, which on a request bounds the whole
/// call: [`idle_timeout`](Self::idle_timeout) bounds each silence and
/// [`deadline`](Self::deadline) the whole call. Neither applies unless set, the idle timeout on the
/// call or, for every stream that leaves its own at `Bound::Default`, on the client.
///
/// ```compile_fail,E0599
/// # use std::time::Duration;
/// fn ticks(client: &ulo_rpc::RpcClient) -> ulo_rpc::StreamCall<u64> {
///     client.stream::<_, u64>("clock.ticks", &()).timeout(Duration::from_secs(1))
/// }
/// ```
///
/// ```
/// # use std::time::Duration;
/// # use ulo::Bound;
/// fn ticks(client: &ulo_rpc::RpcClient) -> ulo_rpc::StreamCall<u64> {
///     client.stream::<_, u64>("clock.ticks", &()).idle_timeout(Bound::After(Duration::from_secs(1))).deadline(Duration::from_secs(60))
/// }
/// ```
#[must_use = "a call is sent when awaited"]
pub struct StreamCall<Res> {
    pub(crate) _res: PhantomData<fn() -> Res>,
    pub(crate) spec: Spec,
    idle: Bound,
    deadline: Option<Duration>,
}

impl<Res> StreamCall<Res> {
    fn new(spec: Spec) -> Self {
        StreamCall { _res: PhantomData, spec, idle: Bound::Default, deadline: None }
    }

    /// A header on the call. `deadline-ms` is reserved, as on [`Call::header`]: the client writes
    /// it from [`deadline`](Self::deadline) or [`within`](Self::within), and a call given it here
    /// fails when it is awaited, before it connects, `BadRequest` with the reason
    /// `"reserved_header"`. `traceparent` and `tracestate` follow [`Call::header`]'s rule.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

    /// The longest silence the client tolerates between reply frames, the first counted from when
    /// the call is awaited. A frame not arriving within it ends the stream with `Timeout`, and
    /// dropping the stream then sends `cancel`. It is not sent to the server, and the client's
    /// timeout does not apply to a stream.
    ///
    /// `Bound::Default`, which a call that never sets it has, takes the client's
    /// [`idle_timeout`](RpcClient::idle_timeout), none unless the client set one;
    /// `Bound::Unbounded` is none for this call whatever the client's; `Bound::After(d)` is `d` in
    /// place of the client's. `Bound::After(Duration::ZERO)`, which would end the stream before its
    /// first frame, fails the call when it is awaited, before it connects, `BadRequest` with the
    /// reason `"zero_timeout"` and a [`ZeroTimeout`] as its source.
    pub fn idle_timeout(mut self, idle: Bound) -> Self {
        self.idle = idle;
        self
    }

    /// A bound on the whole call, counted from when it is awaited: the stream ends with `Timeout`
    /// when it passes, and the server is given what is left of it when the call's frame is written
    /// in `deadline-ms`, unless a shorter one applies, and cancels the handler there. None unless
    /// set, since a stream may run indefinitely.
    ///
    /// `Duration::ZERO`, which would end the stream at once, fails the call when it is awaited,
    /// before it connects, `BadRequest` with the reason `"zero_timeout"` and a [`ZeroTimeout`] as
    /// its source, so no request reaches the server.
    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Forwards `exec`'s remaining deadline as `deadline-ms` and its trace context, and cancels the
    /// stream when `exec` is cancelled, as [`Call::within`] does.
    pub fn within(mut self, exec: &ExecutionRef) -> Self {
        self.spec.within = Some(exec.clone());
        self
    }
}

impl<Res: DeserializeOwned + Send + 'static> IntoFuture for StreamCall<Res> {
    type Output = Result<RpcStream<Res>, RpcError>;
    type IntoFuture = BoxFuture<'static, Result<RpcStream<Res>, RpcError>>;

    fn into_future(self) -> Self::IntoFuture {
        let StreamCall { mut spec, idle, deadline, .. } = self;
        Box::pin(async move {
            spec.reserved("bound the stream with `.deadline`, or forward an execution's deadline with `.within`")?;
            let idle = match idle {
                Bound::Default => spec.client.idle,
                Bound::Unbounded => None,
                // The builder answers `Self`, so the await is the first point a zero can fail:
                // before the call connects, as a binary payload on a JSON link is refused.
                Bound::After(Duration::ZERO) => {
                    return Err(ZeroTimeout::of_bound("StreamCall::idle_timeout", spec.client.inner.link, "the stream").refusal());
                }
                Bound::After(idle) => Some(idle),
            };
            if deadline == Some(Duration::ZERO) {
                let instead = "leave `.deadline` unset for no deadline";
                return Err(ZeroTimeout::of_duration("StreamCall::deadline", spec.client.inner.link, "the stream", instead).refusal());
            }
            let ends = spec.limit(Whole::Stream(deadline))?;
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            let exchange = bounded(&*runtime, Limit::frame(idle, ends, runtime.now()), within.as_ref(), spec.open()).await?;
            Ok(RpcStream { items: replies::<Res>(exchange, idle, ends) })
        })
    }
}

/// The reply frames as items, each awaited under `idle` and what is left to `ends`. The exchange
/// is dropped with the stream, which sends `cancel` unless an `end`, an `err` or a single `res`
/// settled it.
fn replies<Res: DeserializeOwned + Send + 'static>(
    exchange: Exchange,
    idle: Option<Duration>,
    ends: Option<Instant>,
) -> BoxStream<'static, Result<Res, RpcError>> {
    let items = futures_util::stream::unfold(Some(exchange), move |exchange| async move {
        let mut exchange = exchange?;
        let timer = Arc::clone(&exchange.timer);
        let within = exchange.within.clone();
        let codec = exchange.codec;
        let next = bounded(&*timer, Limit::frame(idle, ends, timer.now()), within.as_ref(), async { Ok(exchange.replies.next().await) }).await;
        match next {
            Ok(Some(Ok(Frame::Item { data, .. }))) => Some((decode(codec, &data), Some(exchange))),
            // A server whose handler answered one value: the stream is that value.
            Ok(Some(Ok(Frame::Res { data, .. }))) => {
                exchange.pending.settled = true;
                Some((decode(codec, &data), None))
            }
            Ok(Some(Ok(Frame::End { .. }))) => {
                exchange.pending.settled = true;
                None
            }
            Ok(Some(Ok(Frame::Err { error, .. }))) => {
                exchange.pending.settled = true;
                Some((Err(RpcError::from_body(error)), None))
            }
            Ok(Some(Ok(other))) => Some((Err(unexpected(&other)), None)),
            Ok(Some(Err(err))) | Err(err) => Some((Err(err), None)),
            Ok(None) => Some((Err(lost()), None)),
        }
    });
    Box::pin(items)
}

/// A stream of replies: `item` frames decoded as `Res`, ending at `end`, or with one `Err` at an
/// `err`. Dropped before its end, it sends `cancel`.
pub struct RpcStream<Res> {
    pub(crate) items: BoxStream<'static, Result<Res, RpcError>>,
}

impl<Res> Stream for RpcStream<Res> {
    type Item = Result<Res, RpcError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.items.as_mut().poll_next(cx)
    }
}

/// What a caller sees, uniform across links (transports DESIGN §5.2): the server's `err` with its
/// kind and details, or the link's own failure mapped one way everywhere. No responders, a lost
/// link or a broker refusal is `Unavailable`; the client's timeout `Timeout`, and a client whose
/// runtime has shut down while it timed the call, or a link whose runtime refused a wait its
/// connect made, `Unavailable` with `reason: "runtime_shut_down"`;
/// an oversized payload
/// `BadRequest` with `reason: "payload_too_large"`; a binary payload on a JSON link `BadRequest`
/// with `reason: "binary_unsupported"`, before any I/O; a `deadline-ms` the caller set with
/// `.header` `BadRequest` with `reason: "reserved_header"`, before any I/O; a request frame the
/// link cannot encode `Internal`; a connect the link refuses as [`Misconfigured`] `Internal` with
/// `reason: "misconfigured_link"`; a connect that panicked `Unavailable`, the link's error, which
/// holds the redacted panic, as its `source()`, so [`ulo::is_panic`] is true of it. A pattern nothing handles is `Unavailable`
/// with `reason: "pattern_unhandled"` or `"no_destination"` where the link signals it, and the
/// client's `Timeout` where it cannot.
#[derive(Debug)]
pub struct RpcError {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) details: Details,
    /// The client's own refusal, where it has a type: a [`ZeroTimeout`] for a call's zero timeout,
    /// a stream's zero deadline or a stream's zero idle timeout; or the link's error for a connect
    /// that panicked.
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl RpcError {
    pub(crate) fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        RpcError { kind, message: message.into(), details: Details::new(), source: None }
    }

    pub(crate) fn with_reason(mut self, why: &str) -> Self {
        self.details.push(reason(why));
        self
    }

    fn with_source(mut self, source: impl Error + Send + Sync + 'static) -> Self {
        self.source = Some(Box::new(source));
        self
    }

    pub(crate) fn from_body(body: ErrorBody) -> Self {
        RpcError { kind: body.kind, message: body.message, details: body.details, source: None }
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn details(&self) -> &Details {
        &self.details
    }

    /// The `ErrorInfo` detail's `reason`, when one is present: `"no_destination"`,
    /// `"pattern_unhandled"`, `"payload_too_large"`, `"binary_unsupported"`, `"zero_timeout"`,
    /// `"reserved_header"` for a `deadline-ms` the caller set, `"misconfigured_link"` for a link
    /// that refused to connect as [`Misconfigured`], `"runtime_shut_down"` for a call whose
    /// client's runtime refused to time it or whose link's connect answered
    /// [`RuntimeShutDown`](ulo::RuntimeShutDown), and the server's `"bad_deadline"` for a
    /// `deadline-ms` header it cannot read.
    pub fn reason(&self) -> Option<&str> {
        self.details.iter().find_map(|detail| match detail {
            Detail::ErrorInfo { reason, .. } => Some(reason.as_str()),
            _ => None,
        })
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl Error for RpcError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|source| source as &(dyn Error + 'static))
    }
}

impl Classify for RpcError {
    fn classify(&self) -> ErrorKind {
        self.kind
    }

    fn details(&self) -> Details {
        self.details.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::{Capabilities, DeliveryMode, Inbound};

    /// A JSON link whose client side encodes its frames as CBOR: a JSON payload's opening `{` is
    /// a CBOR text string 27 bytes long, which a short payload cannot fill.
    struct MisencodingLink;

    impl Link for MisencodingLink {
        const NAME: &'static str = "misencoding";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            Ok(Outbound {
                send: Box::new(|_pattern, frame, _reply_to| {
                    Box::pin(async move { Codec::Cbor.encode_frame(&frame).map(drop).map_err(BoxError::from) })
                }),
                replies: Box::pin(futures_util::stream::pending()),
            })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// A link whose first connection's reply lane ends at once, as a server closing does, and
    /// whose later ones stay open; it counts its connections.
    struct EndingLink(Arc<AtomicU64>);

    impl Link for EndingLink {
        const NAME: &'static str = "ending";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            let first = self.0.fetch_add(1, Ordering::Relaxed) == 0;
            let replies: BoxStream<'static, Frame> =
                if first { Box::pin(futures_util::stream::empty()) } else { Box::pin(futures_util::stream::pending()) };
            Ok(Outbound { send: Box::new(|_pattern, _frame, _reply_to| Box::pin(async { Ok(()) })), replies })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// A link whose server never answers, recording one header of each opening frame it sends,
    /// `req` or `open`, and `None` for one carrying none: `deadline-ms`, or the header named to
    /// [`SilentLink::recording`].
    #[derive(Clone)]
    struct SilentLink(Arc<Mutex<Vec<Opening>>>, &'static str);

    /// An opening frame's kind and the recorded header's value.
    type Opening = (&'static str, Option<String>);

    impl Default for SilentLink {
        fn default() -> Self {
            SilentLink::recording(DEADLINE_MS)
        }
    }

    impl SilentLink {
        fn recording(header: &'static str) -> Self {
            SilentLink(Arc::default(), header)
        }

        fn sent(&self) -> Vec<Opening> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    impl Link for SilentLink {
        const NAME: &'static str = "silent";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            let (sent, header) = (Arc::clone(&self.0), self.1);
            Ok(Outbound {
                send: Box::new(move |_pattern, frame, _reply_to| {
                    let opening = match &frame {
                        Frame::Req { headers, .. } | Frame::Open { headers, .. } => Some((frame.kind(), headers.get(header).map(str::to_owned))),
                        _ => None,
                    };
                    if let Some(opening) = opening {
                        sent.lock().unwrap_or_else(PoisonError::into_inner).push(opening);
                    }
                    Box::pin(async { Ok(()) })
                }),
                replies: Box::pin(futures_util::stream::pending()),
            })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    fn silent_client(link: &SilentLink, timeout: Duration) -> RpcClient {
        RpcClient::of_module(Arc::new(link.clone()), Bound::After(timeout), Bound::Default, Arc::new(ulo_runtime_tokio::Tokio::current()))
    }

    /// A client over a link whose server never answers, every stream's idle timeout `idle`.
    fn idle_client(idle: Bound) -> RpcClient {
        RpcClient::of_module(Arc::new(SilentLink::default()), Bound::Default, idle, Arc::new(ulo_runtime_tokio::Tokio::current()))
    }

    /// The stream's next item, failing the test after five seconds rather than hanging it.
    async fn next_within_5s(stream: &mut RpcStream<u32>, what: &str) -> Option<Result<u32, RpcError>> {
        tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap_or_else(|_| panic!("{what} did not happen within 5s"))
    }

    /// A stream's deadline is sent as `deadline-ms` on `stream` and `duplex` alike; its idle
    /// timeout is not, and the client's timeout, which a request sends, does not reach a stream.
    /// Each value is what is left of its bound when the frame is written, so the bound itself or a
    /// millisecond under it.
    #[tokio::test]
    async fn a_stream_sends_its_deadline_and_neither_its_idle_timeout_nor_the_client_s_timeout() {
        let link = SilentLink::default();
        let client = silent_client(&link, Duration::from_millis(300));
        let items = || futures_util::stream::iter([1u32]);
        let timed_out = client.request::<_, ()>("silent.request", &()).await.expect_err("nothing answers the request");
        assert_eq!(timed_out.kind(), ErrorKind::Timeout);
        drop(client.stream::<_, u32>("silent.stream", &()).deadline(Duration::from_secs(2)).await.expect("the stream opens"));
        drop(client.duplex::<_, u32>("silent.duplex", items()).deadline(Duration::from_millis(1500)).await.expect("the duplex opens"));
        drop(client.stream::<_, u32>("silent.stream", &()).idle_timeout(Bound::After(Duration::from_secs(1))).await.expect("the stream opens"));
        drop(client.stream::<_, u32>("silent.stream", &()).await.expect("the stream opens"));
        drop(client.duplex::<_, u32>("silent.duplex", items()).await.expect("the duplex opens"));
        let bounds = [("req", Some(300)), ("req", Some(2000)), ("open", Some(1500)), ("req", None), ("req", None), ("open", None)];
        let sent = link.sent();
        let within = sent.len() == bounds.len()
            && sent.iter().zip(bounds).all(|((kind, value), (expected, bound))| {
                let value = value.as_deref().map(|value| value.parse::<u64>().unwrap_or(u64::MAX));
                *kind == expected && value.zip(bound).map_or(value.is_none() && bound.is_none(), |(value, bound)| (bound - 1..=bound).contains(&value))
            });
        assert!(
            within,
            "each opening frame's `deadline-ms`, the request's timeout, two streams' deadlines, then three streams with none, \
             expected at or a millisecond under {bounds:?}: {sent:?}"
        );
    }

    /// The named durations, then a pseudo-random sample spread over every magnitude a `Duration`
    /// holds, the same on every run.
    fn remaining_times() -> Vec<Duration> {
        let mut durations = vec![
            Duration::from_nanos(1),
            Duration::from_nanos(999),
            Duration::from_micros(1) + Duration::from_nanos(1),
            Duration::from_millis(1) - Duration::from_nanos(1),
            Duration::from_millis(1),
            Duration::from_millis(1) + Duration::from_nanos(1),
            Duration::from_secs(1),
            Duration::from_secs(60),
            Duration::from_secs(3600),
            Duration::from_secs(3600 * 24),
            Duration::from_secs(3600 * 24 * 100),
            Duration::from_millis(u64::MAX),
            Duration::from_millis(u64::MAX) + Duration::from_nanos(999_999),
            Duration::from_millis(u64::MAX) + Duration::from_millis(1),
            Duration::from_secs(u64::MAX / 2),
            Duration::MAX,
        ];
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..2000 {
            let nanos = next() >> (next() % 64);
            durations.push(Duration::from_nanos(nanos.max(1)));
            let secs = next() >> (next() % 64);
            durations.push(Duration::new(secs, (next() % 1_000_000_000) as u32));
        }
        durations
    }

    /// Every remaining time is written as a whole number of milliseconds the server reads, never
    /// as zero, and read back less than a millisecond away, or as the longest value the server
    /// reads, `u64::MAX` milliseconds, when it is longer than that.
    #[test]
    fn every_remaining_time_is_written_as_milliseconds_the_server_reads_and_never_as_zero() {
        let longest = Duration::from_millis(u64::MAX);
        let mut failures = Vec::new();
        for remaining in remaining_times() {
            let value = deadline_ms(remaining);
            let mut headers = CallHeaders::new();
            headers.insert(DEADLINE_MS, value.clone());
            match headers.deadline() {
                Err(_) => failures.push(format!("{remaining:?}: written {value:?}, which the server cannot read")),
                Ok(None) => failures.push(format!("{remaining:?}: written {value:?}, read back as no deadline")),
                Ok(Some(decoded)) => {
                    let close = if remaining > longest { decoded == longest } else { remaining.abs_diff(decoded) < Duration::from_millis(1) };
                    if !value.bytes().all(|byte| byte.is_ascii_digit()) || decoded.is_zero() || !close {
                        failures.push(format!("{remaining:?}: written {value:?}, read back as {decoded:?}"));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "{} of the sweep's durations failed, the first ten: {:#?}", failures.len(), &failures[..failures.len().min(10)]);
    }

    /// The tokio runtime's spawning on a clock that stands still until [`Stepped::advance`] moves
    /// it, and whose sleeps never end, so no timer the client arms ends a call first.
    #[derive(Clone)]
    struct Stepped {
        tokio: ulo_runtime_tokio::Tokio,
        start: Instant,
        moved: Arc<Mutex<Duration>>,
    }

    impl Stepped {
        fn new() -> Stepped {
            Stepped { tokio: ulo_runtime_tokio::Tokio::current(), start: Instant::now(), moved: Arc::default() }
        }

        fn advance(&self, by: Duration) {
            *self.moved.lock().unwrap_or_else(PoisonError::into_inner) += by;
        }
    }

    impl Timer for Stepped {
        fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
            Box::pin(std::future::pending())
        }

        fn now(&self) -> Instant {
            self.start + *self.moved.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
            let sleep = Timer::sleep(self, d);
            Box::pin(async move {
                sleep.await;
                Ok(())
            })
        }
    }

    impl ulo::Spawn for Stepped {
        fn spawn(&self, fut: BoxFuture<'static, ()>) -> ulo::TaskHandle {
            ulo::Spawn::spawn(&self.tokio, fut)
        }

        fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<ulo::TaskHandle, ulo::Refused> {
            ulo::Spawn::try_spawn(&self.tokio, fut)
        }
    }

    /// A [`SilentLink`] whose every connect takes `takes` of a [`Stepped`] clock.
    struct SlowConnect {
        link: SilentLink,
        clock: Stepped,
        takes: Duration,
    }

    impl Link for SlowConnect {
        const NAME: &'static str = "slow-connect";

        fn capabilities(&self) -> Capabilities {
            self.link.capabilities()
        }

        async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            self.link.listen(patterns).await
        }

        async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            self.clock.advance(self.takes);
            self.link.connect(runtime).await
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// F424: `deadline-ms` is what is left of a stream's deadline once its connection is made, in
    /// whole milliseconds. A connect taking 2 ms of a 2.5 ms deadline leaves 500 µs, sent as `1`,
    /// the shortest deadline the server reads; one longer than `u64::MAX` milliseconds is sent as
    /// that, the longest. The clock moves only while the client connects, so what is left is
    /// exact.
    #[tokio::test]
    async fn a_deadline_is_sent_as_milliseconds_the_server_reads() {
        let (link, clock) = (SilentLink::default(), Stepped::new());
        let slow = SlowConnect { link: link.clone(), clock: clock.clone(), takes: Duration::from_millis(2) };
        let client = RpcClient::of_module(Arc::new(slow), Bound::After(Duration::from_secs(30)), Bound::Default, Arc::new(clock));
        drop(client.stream::<_, u32>("silent.stream", &()).deadline(Duration::from_micros(2_500)).await.expect("the stream opens"));
        drop(client.stream::<_, u32>("silent.stream", &()).deadline(Duration::from_secs(1 << 62)).await.expect("the stream opens"));
        assert_eq!(link.sent(), vec![("req", Some("1".to_owned())), ("req", Some(u64::MAX.to_string()))], "each stream's `deadline-ms`");
    }

    /// F400: `traceparent` is an ordinary header. A caller sets it with `.header` to propagate
    /// trace context, and the client sends it as set on a request and a stream, refusing neither.
    #[tokio::test]
    async fn a_caller_s_traceparent_is_sent_as_set() {
        const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        let link = SilentLink::recording("traceparent");
        let client = silent_client(&link, Duration::from_millis(50));
        let timed_out = client.request::<_, u32>("silent.request", &()).header("traceparent", TRACEPARENT).await.expect_err("nothing answers the request");
        assert_eq!(timed_out.kind(), ErrorKind::Timeout, "the request was refused rather than sent: {timed_out:?}");
        drop(client.stream::<_, u32>("silent.stream", &()).header("traceparent", TRACEPARENT).await.expect("the stream opens"));
        assert_eq!(link.sent(), vec![("req", Some(TRACEPARENT.to_owned())), ("req", Some(TRACEPARENT.to_owned()))], "each opening frame's `traceparent`");
    }

    /// A stream whose server goes silent ends at its idle timeout, as `Timeout`.
    #[tokio::test]
    async fn a_silent_stream_ends_at_its_idle_timeout() {
        let client = silent_client(&SilentLink::default(), Duration::from_secs(30));
        let started = std::time::Instant::now();
        let mut stream = client.stream::<_, u32>("silent.stream", &()).idle_timeout(Bound::After(Duration::from_millis(100))).await.expect("the stream opens");
        let ended = next_within_5s(&mut stream, "the idle timeout").await.expect("the stream yields its timeout").expect_err("nothing arrived");
        assert_eq!(ended.kind(), ErrorKind::Timeout);
        assert!(ended.message().contains("idle timeout"), "the stream did not end on its idle timeout: {ended:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "the idle timeout took {:?}", started.elapsed());
        assert!(next_within_5s(&mut stream, "the stream's end").await.is_none(), "the stream yielded after its idle timeout");
    }

    /// A stream ends at its deadline, as `Timeout`, however long its idle timeout.
    #[tokio::test]
    async fn a_stream_ends_at_its_deadline_before_a_longer_idle_timeout() {
        let client = silent_client(&SilentLink::default(), Duration::from_secs(30));
        let started = std::time::Instant::now();
        let mut stream = client
            .stream::<_, u32>("silent.stream", &())
            .idle_timeout(Bound::After(Duration::from_secs(30)))
            .deadline(Duration::from_millis(100))
            .await
            .expect("the stream opens");
        let ended = next_within_5s(&mut stream, "the deadline").await.expect("the stream yields its deadline").expect_err("nothing arrived");
        assert_eq!(ended.kind(), ErrorKind::Timeout);
        assert!(ended.message().contains("deadline"), "the stream did not end at its deadline: {ended:?}");
        assert!(started.elapsed() < Duration::from_secs(5), "the deadline took {:?}", started.elapsed());
    }

    /// A stream given neither bound is not ended by the client's timeout.
    #[tokio::test]
    async fn a_stream_with_neither_bound_outlives_the_client_s_timeout() {
        let client = silent_client(&SilentLink::default(), Duration::from_millis(50));
        let mut stream = client.stream::<_, u32>("silent.stream", &()).await.expect("the stream opens");
        let waited = tokio::time::timeout(Duration::from_millis(500), stream.next()).await;
        assert!(waited.is_err(), "a stream with neither bound ended within ten times the client's timeout: {:?}", waited.ok());
    }

    /// The client's idle timeout ends a silent stream that sets none of its own.
    #[tokio::test]
    async fn a_client_wide_idle_timeout_ends_a_silent_stream() {
        let client = idle_client(Bound::After(Duration::from_millis(100)));
        let started = std::time::Instant::now();
        let mut stream = client.stream::<_, u32>("silent.stream", &()).await.expect("the stream opens");
        let ended = next_within_5s(&mut stream, "the client's idle timeout").await.expect("the stream yields its timeout").expect_err("nothing arrived");
        assert_eq!(ended.kind(), ErrorKind::Timeout);
        assert!(ended.message().contains("idle timeout"), "the stream did not end on its idle timeout: {ended:?}");
        assert!(started.elapsed() >= Duration::from_millis(100), "the stream ended {:?} after it opened", started.elapsed());
        let mut duplex = client.duplex::<_, u32>("silent.duplex", futures_util::stream::pending::<u32>()).await.expect("the duplex opens");
        let ended = next_within_5s(&mut duplex, "the client's idle timeout on a duplex").await.expect("the duplex yields its timeout");
        assert_eq!(ended.expect_err("nothing arrived").kind(), ErrorKind::Timeout);
    }

    /// A call's idle timeout replaces the client's, longer or shorter.
    #[tokio::test]
    async fn a_call_s_idle_timeout_overrides_the_client_s() {
        let short = idle_client(Bound::After(Duration::from_millis(50)));
        let mut longer = short.stream::<_, u32>("silent.stream", &()).idle_timeout(Bound::After(Duration::from_secs(30))).await.expect("the stream opens");
        let waited = tokio::time::timeout(Duration::from_millis(500), longer.next()).await;
        assert!(waited.is_err(), "a stream given a 30 s idle timeout ended within 500 ms, at the client's 50 ms: {:?}", waited.ok());

        let long = idle_client(Bound::After(Duration::from_secs(30)));
        let started = std::time::Instant::now();
        let mut shorter = long.stream::<_, u32>("silent.stream", &()).idle_timeout(Bound::After(Duration::from_millis(100))).await.expect("the stream opens");
        let ended = next_within_5s(&mut shorter, "the call's idle timeout").await.expect("the stream yields its timeout").expect_err("nothing arrived");
        assert_eq!(ended.kind(), ErrorKind::Timeout);
        assert!(started.elapsed() < Duration::from_secs(5), "the call's idle timeout took {:?}", started.elapsed());
    }

    /// With no idle timeout given, at `Bound::Default` as from `RpcClient::new`, a stream waits
    /// out an hour of silence on the paused clock, where any timer the client armed would fire
    /// first.
    #[tokio::test(start_paused = true)]
    async fn the_default_idle_timeout_never_ends_a_quiet_stream() {
        let module = idle_client(Bound::Default);
        let built = RpcClient::new(SilentLink::default(), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("the link is usable");
        for (client, which) in [(module, "a module's client"), (built, "a client from `RpcClient::new`")] {
            let mut stream = client.stream::<_, u32>("silent.stream", &()).await.expect("the stream opens");
            let waited = tokio::time::timeout(Duration::from_secs(3600), stream.next()).await;
            assert!(waited.is_err(), "{which}'s quiet stream ended within an hour: {:?}", waited.ok());
        }
    }

    /// The tokio runtime, recording every sleep the client asks of it.
    #[derive(Clone)]
    struct Recording {
        tokio: ulo_runtime_tokio::Tokio,
        sleeps: Arc<Mutex<Vec<Duration>>>,
    }

    impl Recording {
        fn new() -> Recording {
            Recording { tokio: ulo_runtime_tokio::Tokio::current(), sleeps: Arc::default() }
        }

        fn sleeps(&self) -> Vec<Duration> {
            self.sleeps.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    impl Timer for Recording {
        fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
            self.sleeps.lock().unwrap_or_else(PoisonError::into_inner).push(d);
            Timer::sleep(&self.tokio, d)
        }

        fn now(&self) -> Instant {
            Timer::now(&self.tokio)
        }

        fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
            self.sleeps.lock().unwrap_or_else(PoisonError::into_inner).push(d);
            Timer::try_sleep(&self.tokio, d)
        }
    }

    impl ulo::Spawn for Recording {
        fn spawn(&self, fut: BoxFuture<'static, ()>) -> ulo::TaskHandle {
            ulo::Spawn::spawn(&self.tokio, fut)
        }

        fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<ulo::TaskHandle, ulo::Refused> {
            ulo::Spawn::try_spawn(&self.tokio, fut)
        }
    }

    /// A client over a link whose server never answers, on a runtime recording its sleeps.
    fn recorded_client(link: &SilentLink, timeout: Bound, idle: Bound) -> (RpcClient, Recording) {
        let runtime = Recording::new();
        (RpcClient::of_module(Arc::new(link.clone()), timeout, idle, Arc::new(runtime.clone())), runtime)
    }

    /// An hour of the paused clock, which any timer the client armed for a bound it can reach
    /// would outlast.
    const HOUR: Duration = Duration::from_secs(3600);

    /// What a bound whose end an `Instant` cannot hold is held to: still waiting after an hour,
    /// and no sleep asked of the runtime, whose own `now + d` would overflow.
    fn behaves_as_unbounded<T: fmt::Debug>(waited: Result<T, tokio::time::error::Elapsed>, runtime: &Recording, what: &str) {
        assert!(waited.is_err(), "{what} ended within an hour: {:?}", waited.ok());
        assert_eq!(runtime.sleeps(), Vec::<Duration>::new(), "{what}: the sleeps the client asked of its runtime");
    }

    /// The stream `call` opens, waited on for an hour of silence.
    async fn an_hour_on(call: StreamCall<u32>, what: &str) -> Result<Option<Result<u32, RpcError>>, tokio::time::error::Elapsed> {
        let mut stream = call.await.unwrap_or_else(|error| panic!("{what}: the stream did not open: {error:?}"));
        tokio::time::timeout(HOUR, stream.next()).await
    }

    /// A client timeout whose end an `Instant` cannot hold is no timeout, as `Bound::Unbounded`
    /// is: no timer, and no `deadline-ms`.
    #[tokio::test(start_paused = true)]
    async fn an_overflowing_client_timeout_is_no_timeout() {
        let link = SilentLink::default();
        let (client, runtime) = recorded_client(&link, Bound::After(Duration::MAX), Bound::Default);
        let waited = tokio::time::timeout(HOUR, client.request::<_, u32>("silent.request", &()).into_future()).await;
        behaves_as_unbounded(waited, &runtime, "a request on a client timing out at `Duration::MAX`");
        assert_eq!(link.sent(), vec![("req", None)], "the request's `deadline-ms`");
    }

    /// A call's own timeout whose end an `Instant` cannot hold is no timeout, the client's five
    /// seconds included: no timer, and no `deadline-ms`.
    #[tokio::test(start_paused = true)]
    async fn an_overflowing_call_timeout_is_no_timeout() {
        let link = SilentLink::default();
        let (client, runtime) = recorded_client(&link, Bound::Default, Bound::Default);
        let waited = tokio::time::timeout(HOUR, client.request::<_, u32>("silent.request", &()).timeout(Duration::MAX).into_future()).await;
        behaves_as_unbounded(waited, &runtime, "a request given `.timeout(Duration::MAX)`");
        assert_eq!(link.sent(), vec![("req", None)], "the request's `deadline-ms`");
    }

    /// F391: a stream's deadline whose end an `Instant` cannot hold panicked computing it. It is
    /// no deadline: no timer, and no `deadline-ms`.
    #[tokio::test(start_paused = true)]
    async fn an_overflowing_stream_deadline_is_no_deadline() {
        let link = SilentLink::default();
        let (client, runtime) = recorded_client(&link, Bound::Default, Bound::Default);
        let what = "a stream given `.deadline(Duration::MAX)`";
        let waited = an_hour_on(client.stream::<_, u32>("silent.stream", &()).deadline(Duration::MAX), what).await;
        behaves_as_unbounded(waited, &runtime, what);
        assert_eq!(link.sent(), vec![("req", None)], "the stream's `deadline-ms`");
    }

    /// A client idle timeout whose end an `Instant` cannot hold is no idle timeout.
    #[tokio::test(start_paused = true)]
    async fn an_overflowing_client_idle_timeout_is_no_idle_timeout() {
        let (client, runtime) = recorded_client(&SilentLink::default(), Bound::Default, Bound::After(Duration::MAX));
        let what = "a stream on a client whose idle timeout is `Duration::MAX`";
        let waited = an_hour_on(client.stream::<_, u32>("silent.stream", &()), what).await;
        behaves_as_unbounded(waited, &runtime, what);
    }

    /// A call's idle timeout whose end an `Instant` cannot hold is no idle timeout, the client's
    /// included.
    #[tokio::test(start_paused = true)]
    async fn an_overflowing_call_idle_timeout_is_no_idle_timeout() {
        let (client, runtime) = recorded_client(&SilentLink::default(), Bound::Default, Bound::After(Duration::from_millis(100)));
        let what = "a stream given `.idle_timeout(Bound::After(Duration::MAX))`";
        let waited = an_hour_on(client.stream::<_, u32>("silent.stream", &()).idle_timeout(Bound::After(Duration::MAX)), what).await;
        behaves_as_unbounded(waited, &runtime, what);
    }

    /// What a [`Refusing`] link's server does with each `req`.
    #[derive(Clone, Copy)]
    enum Refuses {
        /// Answers it `err` of kind `bad_request`.
        Answer,
        /// Refuses to send it, as a frame over the link's limit.
        Send,
    }

    /// A link counting its connects and every frame it is asked to send, whose server refuses
    /// each `req` as [`Refuses`] says.
    #[derive(Clone)]
    struct Refusing {
        refuses: Refuses,
        connects: Arc<AtomicU64>,
        sends: Arc<Mutex<Vec<&'static str>>>,
    }

    impl Refusing {
        fn new(refuses: Refuses) -> Refusing {
            Refusing { refuses, connects: Arc::default(), sends: Arc::default() }
        }

        /// Connects made and the kinds of the frames the client asked the link to send.
        fn counts(&self) -> (u64, Vec<&'static str>) {
            (self.connects.load(Ordering::SeqCst), self.sends.lock().unwrap_or_else(PoisonError::into_inner).clone())
        }
    }

    impl Link for Refusing {
        const NAME: &'static str = "refusing";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            let (lane, replies) = mpsc::unbounded();
            let (refuses, sends) = (self.refuses, Arc::clone(&self.sends));
            Ok(Outbound {
                send: Box::new(move |_pattern, frame, _reply_to| {
                    sends.lock().unwrap_or_else(PoisonError::into_inner).push(frame.kind());
                    let refused = match (&frame, refuses) {
                        (Frame::Req { id, .. }, Refuses::Answer) => {
                            let error = ErrorBody::new(ErrorKind::BadRequest, "refused", Details::new());
                            let _ = lane.unbounded_send(Frame::Err { id: *id, error });
                            Ok(())
                        }
                        (Frame::Req { .. }, Refuses::Send) => Err(BoxError::from(FrameTooLarge { size: 2, limit: 1 })),
                        _ => Ok(()),
                    };
                    Box::pin(async move { refused })
                }),
                replies: Box::pin(replies),
            })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// The client has no retry policy: every `BadRequest` is final, attempted once whatever made
    /// it. A zero bound is refused before the client connects, a request the server answers
    /// `bad_request` is sent once, and one the link refuses as too large is offered to the link
    /// once; an hour of the paused clock later nothing more has been sent.
    #[tokio::test(start_paused = true)]
    async fn a_bad_request_is_attempted_once() {
        let runtime = || -> Arc<dyn Runtime> { Arc::new(ulo_runtime_tokio::Tokio::current()) };
        let idle = Bound::After(Duration::from_secs(1));
        let link = Refusing::new(Refuses::Answer);
        let client = RpcClient::of_module(Arc::new(link.clone()), Bound::Default, idle, runtime());
        let zeros = [
            ("`StreamCall::idle_timeout(Bound::After(Duration::ZERO))`", client.stream::<_, u32>("refusing.stream", &()).idle_timeout(Bound::After(Duration::ZERO)).await.map(drop)),
            ("`StreamCall::deadline(Duration::ZERO)`", client.stream::<_, u32>("refusing.stream", &()).deadline(Duration::ZERO).await.map(drop)),
            ("`Call::timeout(Duration::ZERO)`", client.request::<_, u32>("refusing.request", &()).timeout(Duration::ZERO).await.map(drop)),
        ];
        for (what, refused) in zeros {
            let refused = refused.expect_err(what);
            assert_eq!((refused.kind(), refused.reason()), (ErrorKind::BadRequest, Some("zero_timeout")), "{what}: {refused:?}");
        }
        tokio::time::sleep(HOUR).await;
        assert_eq!(link.counts(), (0, vec![]), "the connects and frames of three calls refused before connecting");

        let answered = client.request::<_, u32>("refusing.request", &()).await.expect_err("the server refuses the request");
        assert_eq!(answered.kind(), ErrorKind::BadRequest, "the server's answer: {answered:?}");
        tokio::time::sleep(HOUR).await;
        assert_eq!(link.counts(), (1, vec!["req"]), "the connects and frames of a request the server answered `bad_request`");

        let link = Refusing::new(Refuses::Send);
        let client = RpcClient::of_module(Arc::new(link.clone()), Bound::Default, idle, runtime());
        let too_large = client.request::<_, u32>("refusing.request", &()).await.expect_err("the link refuses the request");
        assert_eq!((too_large.kind(), too_large.reason()), (ErrorKind::BadRequest, Some("payload_too_large")), "the link's refusal: {too_large:?}");
        tokio::time::sleep(HOUR).await;
        assert_eq!(link.counts(), (1, vec!["req"]), "the connects and frames of a request the link refused as too large");
    }

    /// The `deadline-ms` values a caller might set: the four the server cannot read, then two it
    /// can.
    const CALLER_DEADLINES: [&str; 6] = ["abc", "18446744073709551615999", "-5", "", "50", "0"];

    /// Asserts `refused` is the client's refusal of a caller-set `deadline-ms`.
    fn reserved_refused(refused: Result<(), RpcError>, what: &str) -> RpcError {
        let refused = refused.expect_err(what);
        assert_eq!((refused.kind(), refused.reason()), (ErrorKind::BadRequest, Some("reserved_header")), "{what}: {refused:?}");
        refused
    }

    /// F395: a `deadline-ms` the caller sets with `.header` is refused when the call is awaited,
    /// before it connects, readable or not, on every call shape: a request on a client whose
    /// timeout the client stamps, one on a client with none, one with its own timeout, a streamed
    /// request, a stream with and without a deadline, a duplex call and an event. The link has
    /// made no connect and sent nothing an hour later.
    #[tokio::test(start_paused = true)]
    async fn a_caller_s_deadline_ms_is_refused_before_the_call_connects() {
        let runtime = || -> Arc<dyn Runtime> { Arc::new(ulo_runtime_tokio::Tokio::current()) };
        let link = Refusing::new(Refuses::Answer);
        let stamping = RpcClient::of_module(Arc::new(link.clone()), Bound::Default, Bound::Default, runtime());
        let unbounded = RpcClient::of_module(Arc::new(link.clone()), Bound::Unbounded, Bound::Default, runtime());
        let items = || futures_util::stream::iter([1u32]);
        for value in CALLER_DEADLINES {
            let calls: [(&str, BoxFuture<'static, Result<(), RpcError>>); 8] = [
                ("a request on a client stamping its timeout", stamping.request::<_, u32>("refusing.request", &()).header("deadline-ms", value).into_future().map(|r| r.map(drop)).boxed()),
                ("a request on a client with no timeout", unbounded.request::<_, u32>("refusing.request", &()).header("deadline-ms", value).into_future().map(|r| r.map(drop)).boxed()),
                ("a request with its own timeout", stamping.request::<_, u32>("refusing.request", &()).header("deadline-ms", value).timeout(Duration::from_secs(1)).into_future().map(|r| r.map(drop)).boxed()),
                ("a streamed request", stamping.send_stream::<_, u32>("refusing.request", items()).header("deadline-ms", value).into_future().map(|r| r.map(drop)).boxed()),
                ("a stream with no deadline", stamping.stream::<_, u32>("refusing.stream", &()).header("deadline-ms", value).into_future().map(|r| r.map(drop)).boxed()),
                ("a stream with a deadline", stamping.stream::<_, u32>("refusing.stream", &()).header("deadline-ms", value).deadline(Duration::from_secs(2)).into_future().map(|r| r.map(drop)).boxed()),
                ("a duplex call", stamping.duplex::<_, u32>("refusing.stream", items()).header("deadline-ms", value).into_future().map(|r| r.map(drop)).boxed()),
                ("an event", stamping.emit("refusing.event", &()).header("deadline-ms", value).into_future().boxed()),
            ];
            for (what, call) in calls {
                reserved_refused(call.await, &format!("{what} carrying `deadline-ms: {value:?}`"));
            }
        }
        tokio::time::sleep(HOUR).await;
        assert_eq!(link.counts(), (0, vec![]), "the connects and frames of calls refused before connecting");
    }

    /// The refusal quotes the caller's value as the server's `bad_deadline` does: whole up to 64
    /// characters, cut to 64 and marked past that.
    #[tokio::test]
    async fn the_reserved_header_refusal_quotes_the_value_cut_short() {
        let client = RpcClient::of_module(Arc::new(Refusing::new(Refuses::Answer)), Bound::Default, Bound::Default, Arc::new(ulo_runtime_tokio::Tokio::current()));
        let short = reserved_refused(client.request::<_, u32>("refusing.request", &()).header("deadline-ms", "abc").await.map(drop), "`abc`");
        assert!(short.message().contains(r#"to "abc" with"#), "the short value is not quoted whole: {short:?}");
        let long = "9".repeat(100);
        let cut = reserved_refused(client.request::<_, u32>("refusing.request", &()).header("deadline-ms", long.as_str()).await.map(drop), "a hundred nines");
        assert!(cut.message().contains(&format!("to {:?}, cut short with", "9".repeat(64))), "the long value is not cut to 64: {cut:?}");
    }

    /// A link whose reply lane ends when the test says, as a lost connection's does.
    #[derive(Clone, Default)]
    struct SeveredLink(Arc<Mutex<Option<UnboundedSender<Frame>>>>);

    impl SeveredLink {
        fn sever(&self) {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        }
    }

    impl Link for SeveredLink {
        const NAME: &'static str = "severed";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            let (lane, replies) = mpsc::unbounded();
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(lane);
            Ok(Outbound { send: Box::new(|_pattern, _frame, _reply_to| Box::pin(async { Ok(()) })), replies: Box::pin(replies) })
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// A lost connection fails a waiting stream `Unavailable` through the reply lane, with no idle
    /// timeout to end it.
    #[tokio::test]
    async fn a_lost_connection_fails_a_stream_with_no_idle_timeout() {
        let link = SeveredLink::default();
        let client = RpcClient::of_module(Arc::new(link.clone()), Bound::Default, Bound::Unbounded, Arc::new(ulo_runtime_tokio::Tokio::current()));
        let mut stream = client.stream::<_, u32>("severed.stream", &()).await.expect("the stream opens");
        link.sever();
        let ended = next_within_5s(&mut stream, "the lost connection's failure").await.expect("the stream yields the loss").expect_err("nothing arrived");
        assert_eq!(ended.kind(), ErrorKind::Unavailable, "what a lost connection fails a stream with: {ended:?}");
    }

    /// F364: a call handed a connection whose reply lane then ended, before the call registered,
    /// waited for its own timeout, since the lane had already failed every call registered on it.
    #[tokio::test(flavor = "current_thread")]
    async fn a_call_is_not_registered_on_a_connection_whose_reply_lane_ended() {
        let connects = Arc::new(AtomicU64::new(0));
        let link = EndingLink(Arc::clone(&connects));
        let client = RpcClient::of_module(Arc::new(link), Bound::Unbounded, Bound::Default, Arc::new(ulo_runtime_tokio::Tokio::current()));
        let handed = client.inner.connection().await.expect("the first connection opens");
        // The reply lane's task runs once this task yields, and ends with its empty stream.
        for _ in 0..100 {
            if !handed.open.load(Ordering::Acquire) {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(!handed.open.load(Ordering::Acquire), "the first connection's reply lane did not end");

        let (waiting, _replies) = mpsc::unbounded();
        assert!(!handed.register(1, waiting), "a call was registered on a connection whose reply lane had ended");
        assert!(handed.pending().is_empty(), "the ended connection holds a call no reply can reach");

        let (conn, _, _, _) = client.inner.register().await.expect("the call registers on a new connection");
        assert!(!Arc::ptr_eq(&conn, &handed), "the call was registered on the ended connection");
        assert_eq!(connects.load(Ordering::Relaxed), 2, "the call did not go over a new connection");
    }

    #[tokio::test]
    async fn a_request_frame_the_link_cannot_encode_is_internal() {
        let client = RpcClient::of_module(Arc::new(MisencodingLink), Bound::Unbounded, Bound::Default, Arc::new(ulo_runtime_tokio::Tokio::current()));
        let answer = client.request::<_, serde_json::Value>("orders.create", &serde_json::json!({ "id": 1 })).await;
        let err = answer.expect_err("the link refused to encode the request, so the call must fail");
        assert_eq!(err.kind(), ErrorKind::Internal, "expected `Internal` for an unencodable request frame, got: {err:?}");
        assert!(err.message().contains("the Cbor codec cannot encode the frame"), "the message should name the codec's failure, got: {err:?}");
    }

    /// A link whose every connect fails: with a [`Misconfigured`] refusal, a
    /// [`RuntimeShutDown`](ulo::RuntimeShutDown), a [`ConnectTimedOut`], or any other error.
    struct FailsToConnect {
        fails: Fails,
        connects: Arc<AtomicU64>,
    }

    #[derive(Clone, Copy, Debug)]
    enum Fails {
        Misconfigured,
        ShutDown,
        TimedOut,
        Other,
        Panicked,
    }

    /// A connect task's panic, kept as the link's error's source.
    #[derive(Debug)]
    struct Panicked(Arc<ulo::Redacted>);

    impl fmt::Display for Panicked {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "the connect panicked: {}", self.0)
        }
    }

    impl Error for Panicked {
        fn source(&self) -> Option<&(dyn Error + 'static)> {
            Some(&*self.0)
        }
    }

    impl Link for FailsToConnect {
        const NAME: &'static str = "failing";

        fn capabilities(&self) -> Capabilities {
            Capabilities::new(DeliveryMode::Addressed)
        }

        async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
            Err("this link has no server side".into())
        }

        async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            match self.fails {
                Fails::Misconfigured => Err(Misconfigured { source: "no root certificate loaded".into() }.into()),
                Fails::ShutDown => Err(ulo::RuntimeShutDown::new(ulo::Refused).into()),
                Fails::TimedOut => Err(ConnectTimedOut::new(Duration::from_secs(3), "127.0.0.1:4222", "NATS").into()),
                Fails::Other => Err("connection refused".into()),
                Fails::Panicked => match ulo::spawn_with(&ulo_runtime_tokio::Tokio::current(), async { panic!("connecting to amqp://ulo:hunter2@127.0.0.1:1") }).await {
                    Err(ulo::TaskEnd::Panicked(message)) => Err(Box::new(Panicked(message))),
                    other => panic!("the connect task did not panic: {other:?}"),
                },
            }
        }

        async fn drain(&self) {}

        async fn close(&self) -> Result<(), BoxError> {
            Ok(())
        }
    }

    /// A connect the link refuses as `Misconfigured` fails each call `Internal` with
    /// `reason: "misconfigured_link"`, the link's cause in its message, one answered
    /// `RuntimeShutDown` `Unavailable` with `reason: "runtime_shut_down"`, one answered
    /// `ConnectTimedOut` `Unavailable` with `reason: "connect_timed_out"`, and any other failed
    /// connect `Unavailable`. Each call asks the link again.
    #[tokio::test]
    async fn a_misconfigured_link_fails_every_call_internal() {
        for (fails, expected, cause) in [
            (Fails::Misconfigured, (ErrorKind::Internal, Some("misconfigured_link")), "no root certificate loaded"),
            (Fails::ShutDown, (ErrorKind::Unavailable, Some(ulo::RUNTIME_SHUT_DOWN)), "the runtime has shut down"),
            (
                Fails::TimedOut,
                (ErrorKind::Unavailable, Some("connect_timed_out")),
                "the NATS broker at 127.0.0.1:4222 did not answer within the link's `connect_within` of 3s",
            ),
            (Fails::Other, (ErrorKind::Unavailable, None), "connection refused"),
        ] {
            let connects = Arc::new(AtomicU64::new(0));
            let link = FailsToConnect { fails, connects: Arc::clone(&connects) };
            let client = RpcClient::new(link, Arc::new(ulo_runtime_tokio::Tokio::current())).expect("the link is usable");
            for call in 0..2 {
                let failed = client.request::<_, u32>("failing.request", &()).await.expect_err("the link never connects");
                assert_eq!((failed.kind(), failed.reason()), expected, "call {call}, {fails:?}: {failed:?}");
                assert!(failed.message().contains(cause), "call {call} does not name the link's cause: {failed:?}");
            }
            assert_eq!(connects.load(Ordering::SeqCst), 2, "each call asks the link to connect, {fails:?}");
        }
    }

    /// Q21-P6: a connect whose task panicked fails each call `Unavailable` holding the panic, so
    /// `ulo::is_panic` is true of the call's error, the url's credentials redacted. Planted break:
    /// the link's error not kept as the call's source.
    #[tokio::test]
    async fn a_connect_that_panicked_fails_every_call_holding_the_panic() {
        let client = RpcClient::new(FailsToConnect { fails: Fails::Panicked, connects: Arc::default() }, Arc::new(ulo_runtime_tokio::Tokio::current()))
            .expect("the link is usable");
        for call in 0..2 {
            let failed = client.request::<_, u32>("failing.request", &()).await.expect_err("the link never connects");
            assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, None), "call {call}: {failed:?}");
            let message = failed.message().to_owned();
            assert!(message.contains("amqp://[redacted]@127.0.0.1:1") && !message.contains("hunter2"), "call {call} is not redacted: {message}");
            assert!(ulo::is_panic(&BoxError::from(failed)), "call {call} does not hold the panic: {message}");
        }
    }

    /// A tokio runtime that has shut down, as a client's runtime: it refuses every sleep.
    fn shut_down_tokio() -> ulo_runtime_tokio::Tokio {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime builds");
        let handle = runtime.handle().clone();
        runtime.shutdown_background();
        ulo_runtime_tokio::Tokio::from_handle(handle)
    }

    /// N2: a call whose client's runtime has shut down cannot be timed, and fails `Unavailable`
    /// naming the refusal rather than `Timeout`, a request and a stream's wait for a frame alike:
    /// no time passed. The stream's opening, written in its first poll, keeps its answer.
    #[tokio::test]
    async fn a_call_timed_on_a_shut_down_runtime_fails_unavailable_naming_the_refusal() {
        let client = RpcClient::of_module(Arc::new(SilentLink::default()), Bound::After(HOUR), Bound::After(HOUR), Arc::new(shut_down_tokio()));
        let request = tokio::time::timeout(Duration::from_secs(5), client.request::<_, u32>("silent.request", &())).await;
        let mut stream = client.stream::<_, u32>("silent.stream", &()).await.expect("the stream's opening is written in its first poll");
        let frame = tokio::time::timeout(Duration::from_secs(5), stream.next()).await;
        let frame = frame.map(|item| item.expect("the stream yields its end").map(drop));
        for (what, failed) in [("the request", request.map(|r| r.map(drop))), ("the stream's wait for a frame", frame)] {
            let failed = failed.unwrap_or_else(|_| panic!("{what} on a shut-down runtime did not end within 5s")).expect_err(what);
            assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, Some(ulo::RUNTIME_SHUT_DOWN)), "{what}: {failed:?}");
            assert!(failed.message().contains("runtime"), "{what} does not name the runtime: {failed:?}");
            assert!(failed.source().is_some_and(|source| source.is::<ulo::Refused>()), "{what} does not carry the runtime's refusal: {failed:?}");
        }
    }

    /// A clock whose bound has passed by the time its runtime refuses the sleep: the sleep moves
    /// the clock by its length, then refuses.
    struct RefusedLate {
        start: Instant,
        moved: Arc<Mutex<Duration>>,
    }

    impl Timer for RefusedLate {
        fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
            *self.moved.lock().unwrap_or_else(PoisonError::into_inner) += d;
            Box::pin(std::future::ready(()))
        }

        fn now(&self) -> Instant {
            self.start + *self.moved.lock().unwrap_or_else(PoisonError::into_inner)
        }

        fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
            *self.moved.lock().unwrap_or_else(PoisonError::into_inner) += d;
            Box::pin(std::future::ready(Err(ulo::Refused)))
        }
    }

    /// A bound that elapsed on the client's clock before its runtime refused the sleep answers
    /// `Timeout`: the refusal is reported only where no bound had passed.
    #[tokio::test]
    async fn a_bound_passed_before_the_runtime_refuses_answers_timeout() {
        let timer = RefusedLate { start: Instant::now(), moved: Arc::default() };
        let limit = Limit::call(Some(timer.now() + HOUR));
        let answered = bounded(&timer, limit, None, std::future::pending::<Result<u32, RpcError>>()).await;
        assert_eq!(answered.as_ref().map_err(RpcError::kind).err(), Some(ErrorKind::Timeout), "{answered:?}");
    }

    /// Work ready in the poll the runtime's refusal arrives in keeps its answer.
    #[tokio::test]
    async fn work_ready_as_the_runtime_refuses_its_bound_keeps_its_answer() {
        let timer = shut_down_tokio();
        let limit = Limit::call(Some(timer.now() + HOUR));
        for round in 0..64 {
            let answered = bounded(&timer, limit, None, std::future::ready(Ok::<u32, RpcError>(7))).await;
            assert_eq!(answered.as_ref().ok(), Some(&7), "round {round}: the refusal outranked a ready answer: {answered:?}");
        }
    }
}
