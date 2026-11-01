//! The per-call queues (transports DESIGN §5.2, flow control): the client's queue of one call's
//! reply items and the server's of one streamed request's items, each held to `max_queued_items`
//! with no window on the wire. An item past the limit ends that call `Unavailable` with the reason
//! `overflow`, and whoever was producing is told to stop: the client sends `cancel` to a streaming
//! server, the server's `err` ends a client's pump.
//!
//! The end-to-end tests run over the TCP link. The one that counts a server's queue exactly runs
//! over a link the test feeds frame by frame, since over a socket the items a client has sent
//! and the items the server has queued differ by whatever is in flight.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{Notify, mpsc};
use ulo::app::Bound as Listening;
use ulo::{App, AppHandle, BoxError, CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, StartupError, injectable, routes};
use ulo_rpc::link::Inbound as Deliveries;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, Frame, Inbound, Link, Outbound, Pattern, Payload, ReplyPath, RpcClient,
    RpcClientModule, RpcError, ZeroCount,
};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, Count, ErrorKind};

const PATIENCE: Duration = Duration::from_secs(10);

/// The limit the tests that overflow a queue set, small so the count is read item by item.
const LIMIT: u32 = 8;

const FLOOD: &str = "queue.flood";
const IGNORE: &str = "queue.ignore";
const COUNT: &str = "queue.count";
const SUM: &str = "queue.sum";
const ECHO: &str = "queue.echo";
const HANDED: &str = "queue.handed";
const STRICT: &str = "queue.strict";
const LENIENT: &str = "queue.lenient";
const RELAY: &str = "queue.relay";

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// The failure a handler answers with when its request ended failing.
#[derive(Debug)]
struct Cut(String);

impl fmt::Display for Cut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the request was cut short: {}", self.0)
    }
}

impl Error for Cut {}

impl Classify for Cut {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// What the handlers record: how many items the flood produced, how many calls reached the
/// handler that ignores its request, and the cancel reason of the execution whose handler or
/// producer was dropped, `Some(None)` for one dropped uncancelled.
#[derive(Clone, Default)]
struct Seen(Arc<SeenState>);

#[derive(Default)]
struct SeenState {
    produced: AtomicU64,
    ignoring: AtomicU64,
    dropped: Mutex<Option<Option<CancelReason>>>,
    /// What the task reading `queue.handed`'s request read last before its loop ended: `None`
    /// until it ends, `Some(true)` when an item failed.
    handed: Mutex<Option<bool>>,
    /// Raised once `queue.handed` has handed its request over.
    handed_over: Notify,
    /// Items the tasks reading `queue.strict`'s and `queue.lenient`'s requests have read.
    read: AtomicU64,
    /// The failures those tasks read, every one with its kind, as it read it.
    failures: Mutex<Vec<(ErrorKind, String)>>,
}

impl Seen {
    fn produced(&self) -> u64 {
        self.0.produced.load(Ordering::SeqCst)
    }

    fn ignoring(&self) -> u64 {
        self.0.ignoring.load(Ordering::SeqCst)
    }

    fn dropped(&self) -> Option<Option<CancelReason>> {
        *lock(&self.0.dropped)
    }
}

/// Records its execution's cancel reason into [`Seen`] when dropped, with its producer.
struct Recorder {
    exec: ExecutionRef,
    seen: Seen,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        *lock(&self.seen.0.dropped) = Some(self.exec.cancel_reason());
    }
}

#[injectable]
struct Queues {
    seen: Dep<Seen>,
}

#[routes]
impl Queues {
    /// Yields `0, 1, 2, ...` as fast as the link takes them, until its stream is dropped.
    #[ulo_rpc::message("queue.flood")]
    async fn flood(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, Never>> {
        let recorder = Recorder { exec, seen: (*self.seen).clone() };
        stream::unfold(recorder, |recorder| async move {
            let n = recorder.seen.0.produced.fetch_add(1, Ordering::SeqCst);
            Some((Ok(n), recorder))
        })
    }

    /// Reads none of its streamed request and answers nothing, so only its execution's
    /// cancellation ends it.
    #[ulo_rpc::message("queue.ignore")]
    async fn ignore(&self, items: Inbound<u64>, exec: ExecutionRef) -> Result<u64, Never> {
        let _items = items;
        let _recorder = Recorder { exec, seen: (*self.seen).clone() };
        self.seen.0.ignoring.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }

    /// Hands its request to a spawned task, as a bidi handler feeding a channel does, and answers
    /// nothing.
    #[ulo_rpc::message("queue.handed")]
    async fn handed(&self, items: Inbound<u64>) -> Result<u64, Never> {
        let seen = (*self.seen).clone();
        tokio::spawn(async move {
            let mut items = items;
            let mut failed = false;
            while let Some(item) = items.next().await {
                failed = item.is_err();
            }
            *lock(&seen.0.handed) = Some(failed);
        });
        self.seen.0.handed_over.notify_one();
        std::future::pending().await
    }

    /// Hands its request to a spawned task and answers with what that task read: the sum of the
    /// items, or the failure that ended them.
    #[ulo_rpc::message("queue.strict")]
    async fn strict(&self, items: Inbound<u64>) -> Result<u64, Cut> {
        read_on_another_task(items, (*self.seen).clone(), false).await
    }

    /// As `queue.strict`, skipping a failed item and answering the sum of the rest.
    #[ulo_rpc::message("queue.lenient")]
    async fn lenient(&self, items: Inbound<u64>) -> Result<u64, Cut> {
        read_on_another_task(items, (*self.seen).clone(), true).await
    }

    /// Relays each item it reads and skips a failed one, so its reply ends cleanly when its
    /// request ends failing.
    #[ulo_rpc::message("queue.relay")]
    async fn relay(&self, items: Inbound<u64>) -> impl Stream<Item = Result<u64, Never>> {
        items.filter_map(|item| std::future::ready(item.ok().map(Ok)))
    }

    #[ulo_rpc::message("queue.count")]
    async fn count(&self, n: Payload<u64>) -> impl Stream<Item = Result<u64, Never>> {
        stream::iter((0..n.0).map(Ok))
    }

    #[ulo_rpc::message("queue.sum")]
    async fn sum(&self, items: Inbound<u64>) -> Result<u64, Never> {
        let mut items = items;
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item.expect("every item decodes");
        }
        Ok(total)
    }

    #[ulo_rpc::message("queue.echo")]
    async fn echo(&self, n: Payload<u64>) -> Result<u64, Never> {
        Ok(n.0)
    }
}

/// Reads `items` on a spawned task, recording each item and failure into `seen`, and answers the
/// sum, or the first failure unless `lenient`.
async fn read_on_another_task(items: Inbound<u64>, seen: Seen, lenient: bool) -> Result<u64, Cut> {
    let (verdict, answered) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let mut items = items;
        let mut sum = 0;
        let mut failed = None;
        while let Some(item) = items.next().await {
            match item {
                Ok(n) => {
                    sum += n;
                    seen.0.read.fetch_add(1, Ordering::SeqCst);
                }
                Err(err) => {
                    lock(&seen.0.failures).push((err.classify(), format!("{err:?}")));
                    failed.get_or_insert(Cut(err.to_string()));
                }
            }
        }
        let _ = verdict.send(match failed {
            Some(cut) if !lenient => Err(cut),
            _ => Ok(sum),
        });
    });
    answered.await.unwrap_or_else(|_| Err(Cut("the reading task ended without a verdict".to_owned())))
}

struct Root(Seen);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Queues>();
    }
}

/// A server over `link` whose streamed requests queue up to `queued` items.
async fn listening<L: Link>(link: L, queued: Count, seen: Seen) -> Result<App<Listening>, StartupError> {
    serving(ulo_rpc::Server::new(link).max_queued_items(queued), seen).await
}

async fn serving<L: Link>(server: ulo_rpc::Server<L>, seen: Seen) -> Result<App<Listening>, StartupError> {
    App::builder(Root(seen)).runtime(Tokio::current()).wire()?.connect().await?.bind(server).listen().await
}

struct Running {
    addr: std::net::SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: RpcClient,
    seen: Seen,
}

/// A TCP server whose streamed requests queue up to `queued` items, and a client of it with the
/// default limit.
async fn start(queued: Count) -> Running {
    started(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")).max_queued_items(queued)).await
}

/// [`start`] with `server` as it is configured.
async fn started(server: ulo_rpc::Server<Tcp>) -> Running {
    let seen = Seen::default();
    let app = serving(server, seen.clone()).await.map_err(Box::new).expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");
    Running { addr, handle, serving, client, seen }
}

impl Running {
    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

/// Polls `done` every millisecond until it holds, failing the test after [`PATIENCE`].
async fn eventually(what: &str, mut done: impl FnMut() -> bool) {
    let waited = tokio::time::timeout(PATIENCE, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "{what} did not happen within {PATIENCE:?}");
}

/// What a stream yields, to its end.
async fn drained<T: Send + 'static>(items: impl Stream<Item = Result<T, RpcError>>) -> Vec<Result<T, RpcError>> {
    tokio::time::timeout(PATIENCE, items.collect::<Vec<_>>()).await.expect("the stream ended in time")
}

/// Asserts `items` are `0..limit` followed by one `Unavailable` whose reason is `overflow`.
fn overflowed_after(items: Vec<Result<u64, RpcError>>, limit: u64) {
    let read: Vec<u64> = items.iter().map_while(|item| item.as_ref().ok().copied()).collect();
    assert_eq!(read, (0..limit).collect::<Vec<_>>(), "the items held before the overflow: {items:?}");
    assert_eq!(items.len() as u64, limit + 1, "the stream did not end at its overflow: {items:?}");
    let error = items.into_iter().last().and_then(Result::err).expect("the stream ends with the overflow");
    assert_eq!((error.kind(), error.reason()), (ErrorKind::Unavailable, Some("overflow")), "{error}");
    assert!(error.message().contains(&format!("{limit} reply items")), "the error names the limit: {error}");
}

/// F547: a server streaming to a caller that reads nothing fills the client's queue to its limit,
/// and the item past it ends the call: the server's producer stops before the caller reads a
/// thing, and the caller then reads exactly the items held and the overflow. The TCP link
/// forgets a call's id as it reads its `cancel`, so a reply already on its way fails first and
/// the server records `Disconnected` rather than `ClientCancelled` when its producer outruns the
/// `cancel`; the unit test beside the client asserts the `cancel` frame itself.
#[tokio::test]
async fn a_reply_stream_nobody_reads_ends_overflow_after_its_limit_and_its_producer_is_cancelled() {
    let running = start(Count::Default).await;
    let client = running.client.clone().max_queued_items(Count::Max(LIMIT)).expect("a limit of eight");
    let items = client.stream::<_, u64>(FLOOD, &()).await.expect("the stream opens");
    let seen = running.seen.clone();
    eventually("the producer's cancellation, with nothing read", || seen.dropped().is_some()).await;
    let ended = seen.dropped().flatten();
    assert!(matches!(ended, Some(CancelReason::ClientCancelled | CancelReason::Disconnected)), "how the producer ended: {ended:?}");
    assert!(seen.produced() > u64::from(LIMIT), "the producer stopped before the queue overflowed: {}", seen.produced());
    overflowed_after(drained(items).await, u64::from(LIMIT));
    running.stop().await;
}

/// The client's queue holds 1,024 reply items when its limit is left at `Count::Default`.
#[tokio::test]
async fn a_client_s_default_limit_is_1024_reply_items() {
    let running = start(Count::Default).await;
    let items = running.client.stream::<_, u64>(FLOOD, &()).await.expect("the stream opens");
    let seen = running.seen.clone();
    eventually("the producer's cancellation, with nothing read", || seen.dropped().is_some()).await;
    overflowed_after(drained(items).await, 1024);
    running.stop().await;
}

/// F548: a streamed request whose handler reads none of it fills the server's queue, and the item
/// past the limit ends the call: the caller is answered `Unavailable` with the reason `overflow`,
/// and its pump stops pulling from the source, which would otherwise run without end. The source
/// is always ready, so the queue may overflow before the call's task first runs, and then the
/// handler never starts; the exact count, and the handler's cancellation, are the fed link's
/// test below.
#[tokio::test]
async fn a_streamed_request_its_handler_does_not_read_ends_overflow_and_its_client_stops_sending() {
    let running = start(Count::Max(LIMIT)).await;
    let pulled = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&pulled);
    let source = stream::iter(0u64..).inspect(move |_| {
        counted.fetch_add(1, Ordering::SeqCst);
    });
    let answer = tokio::time::timeout(PATIENCE, running.client.send_stream::<_, u64>(IGNORE, source)).await.expect("answered in time");
    let error = answer.expect_err("the overflow ends the call");
    assert_eq!((error.kind(), error.reason()), (ErrorKind::Unavailable, Some("overflow")), "{error}");
    assert!(error.message().contains("`max_queued_items`"), "the error names the setting: {error}");
    let after = pulled.load(Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(pulled.load(Ordering::SeqCst), after, "the client kept pulling items once the call had ended");
    running.stop().await;
}

/// A request handed to another task ends there too when its queue overflows, and ends failing: a
/// reader outside the call's task is not woken by the call's cancellation, and must not read the
/// request as complete.
#[tokio::test]
async fn a_request_read_on_another_task_ends_failing_at_its_overflow() {
    let running = start(Count::Max(LIMIT)).await;
    // The flood waits for the handler: a queue that overflows before the call's task first runs
    // ends the call with its handler never started, and so with nothing handed over.
    let seen = running.seen.clone();
    let handed_over = stream::once(async move { seen.0.handed_over.notified().await }).filter_map(|()| std::future::ready(None));
    let source = stream::iter([0u64]).chain(handed_over).chain(stream::iter(1u64..));
    let answer = tokio::time::timeout(PATIENCE, running.client.send_stream::<_, u64>(HANDED, source)).await.expect("answered in time");
    assert_eq!(answer.expect_err("the overflow ends the call").reason(), Some("overflow"));
    let seen = running.seen.clone();
    eventually("the reading task's end", || lock(&seen.0.handed).is_some()).await;
    assert_eq!(*lock(&running.seen.0.handed), Some(true), "the reading task saw its request end cleanly");
    running.stop().await;
}

/// The server's queue holds 1,024 items of a streamed request when `max_queued_items` is not set.
#[tokio::test]
async fn a_server_s_default_limit_is_1024_request_items() {
    let running = started(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0"))).await;
    let answer = tokio::time::timeout(PATIENCE, running.client.send_stream::<_, u64>(IGNORE, stream::iter(0u64..))).await.expect("answered in time");
    let error = answer.expect_err("the overflow ends the call");
    assert_eq!((error.kind(), error.reason()), (ErrorKind::Unavailable, Some("overflow")), "{error}");
    assert!(error.message().contains("had 1024 items"), "the error names the default limit: {error}");
    running.stop().await;
}

/// A provider holding the client a module binds.
#[injectable]
struct Holder {
    client: Dep<RpcClient>,
}

/// Imports a client module of the server at its address with a limit of [`LIMIT`].
struct Holding(std::net::SocketAddr);

impl Module for Holding {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Tcp::new(self.0)).max_queued_items(Count::Max(LIMIT)));
        m.provide::<Holder>();
    }
}

/// The limit set on `RpcClientModule` is the limit of the client it binds.
#[tokio::test]
async fn a_client_module_s_limit_is_its_client_s() {
    let running = start(Count::Default).await;
    let app = App::builder(Holding(running.addr)).runtime(Tokio::current()).wire().expect("the client wires").connect().await.expect("the client connects");
    let holder = app.get::<Holder>().await.expect("the provider resolves");
    let items = holder.client.stream::<_, u64>(FLOOD, &()).await.expect("the stream opens");
    let seen = running.seen.clone();
    eventually("the producer's cancellation, with nothing read", || seen.dropped().is_some()).await;
    overflowed_after(drained(items).await, u64::from(LIMIT));
    let _ = app.close(Signal::new("test")).await;
    running.stop().await;
}

/// A caller and a handler that read as items arrive move ten times the default limit, in both
/// directions, with no overflow.
#[tokio::test]
async fn readers_that_keep_up_move_ten_times_the_default_limit() {
    let running = start(Count::Default).await;
    let total = 10 * 1024u64;
    let items = running.client.stream::<_, u64>(COUNT, &total).await.expect("the stream opens");
    let read: Vec<u64> = drained(items).await.into_iter().map(|item| item.expect("no item overflowed")).collect();
    assert_eq!(read, (0..total).collect::<Vec<_>>(), "every reply item arrived in order");
    let sum = tokio::time::timeout(PATIENCE, running.client.send_stream::<_, u64>(SUM, stream::iter(0..total))).await.expect("answered in time");
    assert_eq!(sum.expect("no request item overflowed"), total * (total - 1) / 2, "every request item reached the handler");
    running.stop().await;
}

/// A limit of one leaves a call answered by one reply as it was, and a stream of one item still
/// takes its `end`, which fits past the limit as a call's last frame.
#[tokio::test]
async fn a_limit_of_one_leaves_a_unary_call_and_a_stream_s_end_alone() {
    let running = start(Count::Max(1)).await;
    let client = running.client.clone().max_queued_items(Count::Max(1)).expect("a limit of one");
    let echoed = tokio::time::timeout(PATIENCE, client.request::<_, u64>(ECHO, &7u64)).await.expect("answered in time");
    assert_eq!(echoed.expect("the unary call is answered"), 7);
    let items = client.stream::<_, u64>(COUNT, &1u64).await.expect("the stream opens");
    let read: Vec<u64> = drained(items).await.into_iter().map(|item| item.expect("the one item and the end fit")).collect();
    assert_eq!(read, vec![0]);
    let sum = tokio::time::timeout(PATIENCE, client.send_stream::<_, u64>(SUM, stream::iter([5u64]))).await.expect("answered in time");
    assert_eq!(sum.expect("a one-item request fits a limit of one"), 5);
    running.stop().await;
}

/// `Count::Max(0)` would end every stream at its first item, and is refused where each side's
/// limit is written: the client's builder at once, the client module when the app wires, the
/// server in `prepare`.
#[tokio::test]
async fn a_zero_limit_is_refused_on_the_client_its_module_and_the_server() {
    let client = RpcClient::new(Tcp::new("127.0.0.1:9"), Arc::new(Tokio::current())).expect("a client");
    let refused: ZeroCount = client.max_queued_items(Count::Max(0)).err().expect("the client refuses a zero limit");
    assert_eq!(refused.link(), "tcp");
    assert_eq!(
        refused.to_string(),
        "`RpcClient::max_queued_items(Count::Max(0))` on the tcp link would end every stream at its first reply item; write `Count::Unlimited` to remove the limit"
    );

    let wired = App::builder(ClientRoot(Mutex::new(Some(RpcClientModule::for_root(Tcp::new("127.0.0.1:9")).max_queued_items(Count::Max(0))))))
        .runtime(Tokio::current())
        .wire();
    let error = wired.err().expect("the module refuses a zero limit when the app wires").to_string();
    assert!(error.contains("`RpcClientModule::max_queued_items(Count::Max(0))` on the tcp link"), "{error}");

    let error = listening(Tcp::new("127.0.0.1:0"), Count::Max(0), Seen::default()).await.err().expect("the server refuses a zero limit");
    assert!(matches!(error, StartupError::Configure(_)), "a zero limit is a `Configure` refusal: {error}");
    let text = chain(&error);
    assert!(
        text.contains("`.max_queued_items(Count::Max(0))` would end every streamed request at its first item; write `Count::Unlimited` to remove the limit"),
        "{text}"
    );
}

/// `error` and every source under it, joined.
fn chain(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// Imports one client module, taken once.
struct ClientRoot(Mutex<Option<RpcClientModule<Tcp>>>);

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(module) = lock(&self.0).take() {
            m.import(module);
        }
    }
}

/// A link whose inbound stream the test feeds, telling the test when the server drains it.
struct Fed {
    inbound: Mutex<Option<Deliveries>>,
    drained: Arc<Notify>,
}

impl Link for Fed {
    const NAME: &'static str = "fed";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Deliveries, BoxError> {
        lock(&self.inbound).take().ok_or_else(|| "the fed link listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        Err("the fed link has no client side".into())
    }

    async fn drain(&self) {
        self.drained.notify_one();
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

fn delivery(frame: Frame, reply: Option<ReplyPath>) -> Delivery {
    Delivery { frame, reply, ack: Ack::none() }
}

/// The server's queue holds exactly its limit: the items up to it leave the call running with
/// nothing answered, the one past it answers `overflow` and cancels the handler `Explicit`, and
/// what arrives for the call after that, `in_end` included, is dropped.
#[tokio::test]
async fn a_server_s_queue_holds_exactly_its_limit() {
    let (deliveries, inbound) = mpsc::unbounded_channel::<Delivery>();
    let mut inbound = inbound;
    let drained = Arc::new(Notify::new());
    let link = Fed { inbound: Mutex::new(Some(Box::pin(stream::poll_fn(move |cx| inbound.poll_recv(cx))))), drained: Arc::clone(&drained) };
    let seen = Seen::default();
    let app = listening(link, Count::Max(LIMIT), seen.clone()).await.map_err(Box::new).expect("the server binds");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });

    let sent: Arc<Mutex<Vec<Frame>>> = Arc::default();
    let recording = Arc::clone(&sent);
    let reply = ReplyPath::new(move |frame| {
        lock(&recording).push(frame);
        Box::pin(async { Ok(()) })
    });
    let item = |n: u64| delivery(Frame::In { id: 1, data: Data::new(n.to_string()) }, None);
    let opened = Frame::Open { id: 1, pattern: IGNORE.to_owned(), headers: CallHeaders::new() };
    deliveries.send(delivery(opened, Some(reply))).expect("the server reads its link");
    for n in 0..u64::from(LIMIT) {
        deliveries.send(item(n)).expect("the server reads its link");
    }
    // A unary call after the items is answered once every item before it has been routed.
    let echo = Frame::Req { id: 2, pattern: ECHO.to_owned(), headers: CallHeaders::new(), data: Data::new("3") };
    let echo_sent = Arc::clone(&sent);
    let echo_reply = ReplyPath::new(move |frame| {
        lock(&echo_sent).push(frame);
        Box::pin(async { Ok(()) })
    });
    deliveries.send(delivery(echo, Some(echo_reply))).expect("the server reads its link");
    let answered = Arc::clone(&sent);
    eventually("the unary call's answer", || lock(&answered).iter().any(|frame| frame.id() == Some(2))).await;
    assert!(lock(&sent).iter().all(|frame| frame.id() != Some(1)), "the call was answered at its limit: {:?}", lock(&sent));
    assert_eq!(seen.dropped(), None, "the handler ended with its queue at the limit");

    let seen_entered = seen.clone();
    eventually("the handler's start", || seen_entered.ignoring() == 1).await;
    deliveries.send(item(u64::from(LIMIT))).expect("the server reads its link");
    let ended = Arc::clone(&sent);
    eventually("the overflow's answer", || lock(&ended).iter().any(|frame| frame.id() == Some(1))).await;
    deliveries.send(item(u64::from(LIMIT) + 1)).expect("the server reads its link");
    deliveries.send(delivery(Frame::InEnd { id: 1 }, None)).expect("the server reads its link");
    eventually("the handler's end", || seen.dropped().is_some()).await;
    assert_eq!(seen.dropped(), Some(Some(CancelReason::Explicit)), "how the handler's execution ended");
    let answers: Vec<Frame> = lock(&sent).iter().filter(|frame| frame.id() == Some(1)).cloned().collect();
    let [Frame::Err { error, .. }] = answers.as_slice() else { panic!("the call's one answer is an `err`: {answers:?}") };
    let reason = error.details.iter().find_map(|detail| match detail {
        ulo_transport::Detail::ErrorInfo { reason, .. } => Some(reason.as_str()),
        _ => None,
    });
    assert_eq!((error.kind, reason), (ErrorKind::Unavailable, Some("overflow")), "{error:?}");
    assert!(error.message.contains("had 8 items waiting for its handler"), "the answer names the limit: {error:?}");

    // The inbound stream ends once the drain has begun, as a link's does.
    let closing = tokio::spawn(async move { handle.close(Signal::new("test")).await });
    tokio::time::timeout(PATIENCE, drained.notified()).await.expect("the server drained its link");
    drop(deliveries);
    let _ = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
}

/// A server over a [`Fed`] link whose streamed requests queue up to [`LIMIT`] items, every call
/// answered on one record.
struct Feeding {
    deliveries: mpsc::UnboundedSender<Delivery>,
    sent: Arc<Mutex<Vec<Frame>>>,
    seen: Seen,
    handle: AppHandle,
    drained: Arc<Notify>,
    serving: tokio::task::JoinHandle<()>,
}

impl Feeding {
    async fn start() -> Feeding {
        let (deliveries, mut inbound) = mpsc::unbounded_channel::<Delivery>();
        let drained = Arc::new(Notify::new());
        let link = Fed { inbound: Mutex::new(Some(Box::pin(stream::poll_fn(move |cx| inbound.poll_recv(cx))))), drained: Arc::clone(&drained) };
        let seen = Seen::default();
        let app = listening(link, Count::Max(LIMIT), seen.clone()).await.map_err(Box::new).expect("the server binds");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Feeding { deliveries, sent: Arc::default(), seen, handle, drained, serving }
    }

    /// Opens call 1 on `pattern`.
    fn open(&self, pattern: &str) {
        let recording = Arc::clone(&self.sent);
        let reply = ReplyPath::new(move |frame| {
            lock(&recording).push(frame);
            Box::pin(async { Ok(()) })
        });
        let opened = Frame::Open { id: 1, pattern: pattern.to_owned(), headers: CallHeaders::new() };
        self.deliveries.send(delivery(opened, Some(reply))).expect("the server reads its link");
    }

    /// Sends call 1 the items `items`, with no await between them, so the server routes them all
    /// before any reader runs.
    fn items(&self, items: std::ops::Range<u64>) {
        for n in items {
            let item = Frame::In { id: 1, data: Data::new(n.to_string()) };
            self.deliveries.send(delivery(item, None)).expect("the server reads its link");
        }
    }

    fn answers(&self) -> Vec<Frame> {
        lock(&self.sent).clone()
    }

    async fn stop(self) {
        let handle = self.handle;
        let closing = tokio::spawn(async move { handle.close(Signal::new("test")).await });
        tokio::time::timeout(PATIENCE, self.drained.notified()).await.expect("the server drained its link");
        drop(self.deliveries);
        let _ = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

/// Asserts `frame` is the `err` an overflow answers: `Unavailable`, the reason `overflow`, the
/// message naming the server's limit.
fn is_overflow_answer(frame: &Frame) {
    let Frame::Err { error, .. } = frame else { panic!("the call's answer is not an `err`: {frame:?}") };
    let reason = error.details.iter().find_map(|detail| match detail {
        ulo_transport::Detail::ErrorInfo { reason, .. } => Some(reason.as_str()),
        _ => None,
    });
    assert_eq!((error.kind, reason), (ErrorKind::Unavailable, Some("overflow")), "{error:?}");
    assert!(error.message.contains("had 8 items waiting for its handler"), "the answer names the limit: {error:?}");
}

/// A handler that answers from a request its queue cut short is not the call's answer: the reader,
/// on another task, reads the overflow once as the request's last item and the items it had not
/// read never, and the call is answered `overflow` whether the handler returns the failure or
/// skips it and answers `Ok`. The server routes the burst in one turn, so the reader and the
/// handler run after the halt and before the call's task observes its cancellation.
#[tokio::test]
async fn a_handler_answering_from_a_request_cut_short_is_answered_overflow() {
    for pattern in [STRICT, LENIENT] {
        let feeding = Feeding::start().await;
        feeding.open(pattern);
        feeding.items(0..1);
        let seen = feeding.seen.clone();
        eventually("the reader's first item", || seen.0.read.load(Ordering::SeqCst) == 1).await;
        feeding.items(1..u64::from(LIMIT) + 2);
        eventually("the call's answer", || !feeding.answers().is_empty()).await;
        let answers = feeding.answers();
        let [answer] = answers.as_slice() else { panic!("{pattern}: the call's one answer: {answers:?}") };
        is_overflow_answer(answer);
        let failures = lock(&seen.0.failures).clone();
        assert_eq!(
            failures,
            vec![(ErrorKind::Unavailable, format!("Overflowed {{ param: \"payload\", limit: {LIMIT} }}"))],
            "{pattern}: the reader read the overflow once, as an `Unavailable` failure"
        );
        assert_eq!(seen.0.read.load(Ordering::SeqCst), 1, "{pattern}: the reader read items left unread at the overflow");
        feeding.stop().await;
    }
}

/// A reply relaying a request ends with the overflow, never with the clean `end` the relay gives
/// once its request ends, failing, under it.
#[tokio::test]
async fn a_relayed_reply_ends_overflow_and_not_cleanly() {
    let feeding = Feeding::start().await;
    feeding.open(RELAY);
    feeding.items(0..1);
    eventually("the first relayed item", || !feeding.answers().is_empty()).await;
    feeding.items(1..u64::from(LIMIT) + 2);
    eventually("the call's end", || feeding.answers().len() > 1).await;
    let answers = feeding.answers();
    let [Frame::Item { .. }, last] = answers.as_slice() else { panic!("the relayed item, then the call's end: {answers:?}") };
    is_overflow_answer(last);
    feeding.stop().await;
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
