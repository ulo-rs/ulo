//! The RPC client (transports DESIGN §5.4): requests, events and the streamed shapes over a link's
//! client side, connected lazily on the first call.
//!
//! ```ignore
//! self.billing.request::<_, Invoice>("invoices.create", &NewInvoice::from(order))
//!     .header("tenant", order.tenant())
//!     .timeout(Duration::from_secs(2))
//!     .await
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
use ulo::{Bound, BoxError, BoxFuture, CancelReason, ExecutionRef, Runtime, Timer};
use ulo_transport::{Classify, Detail, Details, ErrorKind};

use crate::codec::Codec;
use crate::dispatch::{quote, reason};
use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{FrameTooLarge, FrameUnencodable, Link, NoDestination, Outbound, Pattern, ReplyTo};
use crate::transport::CallHeaders;

/// A client's timeout at `Bound::Default`.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);

/// The header the client writes from a call's bound and the server reads as its deadline. No ulo
/// path copies an incoming call's headers into an outgoing one, so a caller setting it with
/// `.header` is a mistake, refused when the call is awaited.
const DEADLINE_MS: &str = "deadline-ms";

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
/// link built outside a runtime and given none. [`RpcClient::new`] answers it, and
/// `RpcClientModule`'s init hook fails the app's `connect` with it.
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
    /// ```ignore
    /// let billing = RpcClient::new(ulo_rpc_tcp::Tcp::new("billing:7000"), Arc::new(ulo_runtime_tokio::Tokio::current()))?
    ///     .timeout(Bound::After(Duration::from_secs(2)))?;
    /// ```
    ///
    /// A link that can never connect is refused here as [`UnusableLink`], where the mistake is
    /// made, rather than failing each call `Unavailable`: [`Link::usable`] decides, a tokio-based
    /// link built outside a runtime and given none. Nothing else about the link is checked before
    /// its first call.
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
    /// leaves it unbounded. This handle and the clones made from it after carry it; clones made
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
}

/// What bounds a call as a whole, and whether the bound is sent as `deadline-ms`.
#[derive(Clone, Copy)]
enum Whole {
    /// A call answered by one reply: its own timeout or the client's, sent. The client abandons
    /// the call there, so the server stops there too.
    Reply,
    /// An event: the client's timeout, covering the publish and not sent, since the handler runs
    /// with no caller waiting.
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
        Spec { client: client.clone(), pattern: Pattern::from(pattern), request, headers: CallHeaders::new(), timeout: None, within: None }
    }

    /// The wait the whole call is bounded by, `whole`'s bound and the calling execution's
    /// remaining time when that is shorter, and the instant it ends. Refuses a call whose
    /// execution's deadline has passed, before any I/O.
    ///
    /// Stamps `deadline-ms`, the deadline the server enforces, with the shorter of the calling
    /// execution's remaining time and `whole`'s bound unless it covers only an event's publish.
    fn limit(&mut self, whole: Whole) -> Result<(Option<Duration>, Option<Instant>), RpcError> {
        let inner = &self.client.inner;
        let now = inner.runtime.now();
        let own = match whole {
            Whole::Reply | Whole::Publish => self.timeout.or(self.client.timeout),
            Whole::Stream(deadline) => deadline,
        };
        // A bound whose end an `Instant` cannot hold is none, as `Bound::Unbounded` is: `now +
        // own` would panic, and a timer handed it computes the same sum.
        let own = own.filter(|own| now.checked_add(*own).is_some());
        let sent = !matches!(whole, Whole::Publish);
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
        // A `deadline-ms` the caller set was refused by `reserved`, so the stamp is the only one.
        let stamped = [remaining, own.filter(|_| sent)].into_iter().flatten().min();
        if let Some(stamped) = stamped {
            self.headers.insert(DEADLINE_MS, stamped.as_millis().max(1).to_string());
        }
        let after = match (own, remaining) {
            (Some(own), Some(remaining)) => Some(own.min(remaining)),
            (own, remaining) => own.or(remaining),
        };
        Ok((after, after.and_then(|after| now.checked_add(after))))
    }

    /// Refuses a call whose caller set `deadline-ms` with `.header`, readable or not: the client
    /// writes that header from the call's bound, so a caller's value either contradicts the bound
    /// or, where the call has none, sets one the client does not enforce. `BadRequest`, reason
    /// `"reserved_header"`, the first such value quoted as the server's `bad_deadline` quotes one;
    /// `instead` says what to write in its place.
    fn reserved(&self, instead: &str) -> Result<(), RpcError> {
        let Some(value) = self.headers.get(DEADLINE_MS) else { return Ok(()) };
        let message = format!(
            "the call set `{DEADLINE_MS}` to {} with `.header`, and the client reserves that header, writing it from the call's bound; {instead}",
            quote(value)
        );
        Err(RpcError::new(ErrorKind::BadRequest, message).with_reason("reserved_header"))
    }

    /// Connects, registers the call, and sends its opening frame; a streamed request's items follow
    /// from a task of their own.
    async fn open(self) -> Result<Exchange, RpcError> {
        let Spec { client, pattern, request, headers, within, .. } = self;
        let inner = &client.inner;
        let request = match request {
            Request::One(payload) => Request::One(Ok(payload?)),
            stream => stream,
        };
        let (conn, id, waiting, replies) = inner.register().await?;
        let mut pending = Pending { conn: Arc::clone(&conn), id, pattern: pattern.clone(), settled: false };
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
        let Outbound { send, replies } = (self.connect)().await.map_err(|err| {
            RpcError::new(ErrorKind::Unavailable, format!("the {} link could not connect: {err}", self.link))
        })?;
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

/// One wait's bound, and what the caller is told when it passes.
#[derive(Clone, Copy)]
struct Limit {
    after: Duration,
    passed: &'static str,
}

impl Limit {
    /// A call's whole bound, its timeout or its execution's remaining time.
    fn call(after: Option<Duration>) -> Option<Limit> {
        after.map(|after| Limit { after, passed: "the call timed out" })
    }

    /// A stream's wait, from `now`, for its next reply frame: its idle timeout, or the time left
    /// to its deadline at `ends` when that is shorter. An idle timeout whose end an `Instant`
    /// cannot hold is none, as `Bound::Unbounded` is, since the timer would compute `now + idle`.
    fn frame(idle: Option<Duration>, ends: Option<Instant>, now: Instant) -> Option<Limit> {
        const IDLE: &str = "no reply frame arrived within the stream's idle timeout";
        const DEADLINE: &str = "the stream's deadline passed";
        let idle = idle.filter(|idle| now.checked_add(*idle).is_some());
        let left = ends.map(|ends| ends.saturating_duration_since(now));
        match (idle, left) {
            (Some(idle), Some(left)) if left < idle => Some(Limit { after: left, passed: DEADLINE }),
            (Some(idle), _) => Some(Limit { after: idle, passed: IDLE }),
            (None, Some(left)) => Some(Limit { after: left, passed: DEADLINE }),
            (None, None) => None,
        }
    }
}

/// `work` bounded by `limit` on the client's runtime and by the calling execution's cancellation.
async fn bounded<T>(
    timer: &dyn Timer,
    limit: Option<Limit>,
    within: Option<&ExecutionRef>,
    work: impl Future<Output = Result<T, RpcError>>,
) -> Result<T, RpcError> {
    let passed = limit.map_or("the call timed out", |limit| limit.passed);
    let expired = async {
        match limit {
            Some(limit) => timer.sleep(limit.after).await,
            None => std::future::pending().await,
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
    // Unbiased: when more than one is ready, one is picked at random.
    futures_util::select! {
        out = work.fuse() => out,
        () = expired.fuse() => Err(RpcError::new(ErrorKind::Timeout, passed)),
        reason = cancelled.fuse() => Err(match reason {
            Some(CancelReason::Deadline) => RpcError::new(ErrorKind::Timeout, "the calling execution's deadline passed"),
            _ => RpcError::new(ErrorKind::Unavailable, "the calling execution was cancelled"),
        }),
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
    /// `"reserved_header"`.
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

    /// Forwards `exec`'s remaining deadline as `deadline-ms` and cancels the call when `exec` is
    /// cancelled.
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
            let limit = Limit::call(spec.limit(Whole::Reply)?.0);
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
    /// `"reserved_header"`.
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.spec.headers.insert(name, value);
        self
    }

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
            let limit = Limit::call(spec.limit(Whole::Publish)?.0);
            let runtime = Arc::clone(&spec.client.inner.runtime);
            let within = spec.within.clone();
            bounded(&*runtime, limit, within.as_ref(), async move {
                let Spec { client, pattern, request, headers, .. } = spec;
                let data = match request {
                    Request::One(payload) => payload?,
                    Request::Stream(_) => return Err(RpcError::new(ErrorKind::BadRequest, "an event carries one payload")),
                };
                let conn = client.inner.connection().await?;
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
    /// `"reserved_header"`.
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
    /// when it passes, and the server is given it in `deadline-ms` unless a shorter one applies and
    /// cancels the handler there. None unless set, since a stream may run indefinitely.
    ///
    /// `Duration::ZERO`, which would end the stream at once, fails the call when it is awaited,
    /// before it connects, `BadRequest` with the reason `"zero_timeout"` and a [`ZeroTimeout`] as
    /// its source, so no request reaches the server.
    pub fn deadline(mut self, deadline: Duration) -> Self {
        self.deadline = Some(deadline);
        self
    }

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
            let (_, ends) = spec.limit(Whole::Stream(deadline))?;
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
/// link or a broker refusal is `Unavailable`; the client's timeout `Timeout`; an oversized payload
/// `BadRequest` with `reason: "payload_too_large"`; a binary payload on a JSON link `BadRequest`
/// with `reason: "binary_unsupported"`, before any I/O; a `deadline-ms` the caller set with
/// `.header` `BadRequest` with `reason: "reserved_header"`, before any I/O; a request frame the
/// link cannot encode
/// `Internal`. A pattern nothing handles is `Unavailable`
/// with `reason: "pattern_unhandled"` or `"no_destination"` where the link signals it, and the
/// client's `Timeout` where it cannot.
#[derive(Debug)]
pub struct RpcError {
    pub(crate) kind: ErrorKind,
    pub(crate) message: String,
    pub(crate) details: Details,
    /// The client's own refusal, where it has a type: a [`ZeroTimeout`] for a call's zero timeout,
    /// a stream's zero deadline or a stream's zero idle timeout.
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
    /// `"reserved_header"` for a `deadline-ms` the caller set, and the server's `"bad_deadline"`
    /// for a `deadline-ms` header it cannot read.
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

    /// A link whose server never answers, recording the `deadline-ms` of each opening frame it
    /// sends, `req` or `open`, and `None` for one carrying none.
    #[derive(Clone, Default)]
    struct SilentLink(Arc<Mutex<Vec<Opening>>>);

    /// An opening frame's kind and its `deadline-ms`.
    type Opening = (&'static str, Option<String>);

    impl SilentLink {
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
            let sent = Arc::clone(&self.0);
            Ok(Outbound {
                send: Box::new(move |_pattern, frame, _reply_to| {
                    let opening = match &frame {
                        Frame::Req { headers, .. } | Frame::Open { headers, .. } => Some((frame.kind(), headers.get("deadline-ms").map(str::to_owned))),
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
        assert_eq!(
            link.sent(),
            vec![
                ("req", Some("300".to_owned())),
                ("req", Some("2000".to_owned())),
                ("open", Some("1500".to_owned())),
                ("req", None),
                ("req", None),
                ("open", None),
            ],
            "each opening frame's `deadline-ms`: the request's timeout, two streams' deadlines, then three streams with none"
        );
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
    }

    impl ulo::Spawn for Recording {
        fn spawn(&self, fut: BoxFuture<'static, ()>) -> ulo::TaskHandle {
            ulo::Spawn::spawn(&self.tokio, fut)
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
}
