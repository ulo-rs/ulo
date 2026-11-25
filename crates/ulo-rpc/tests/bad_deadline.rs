//! A `deadline-ms` the server cannot read is refused before any handler runs: a request or a
//! streamed request is answered `err` of kind `bad_request` with the reason `bad_deadline` and
//! settled, and an event is rejected. One a server can read still sets the execution's deadline,
//! except zero, a budget already spent, which is answered `timeout` with no handler run and no
//! error handler offered it, as gRPC answers a zero `grpc-timeout` DEADLINE_EXCEEDED.
//!
//! The frames are written by hand and fed to the server through a scripted link, so no client
//! stands between the test and the header the server reads.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::{StreamExt, stream};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use ulo::{AnyErrorHandler, App, AppHandle, BoxError, Dep, ErrorHandler, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, Timer, controller, handlers};
use ulo_rpc::link::Inbound;
use ulo_rpc::{
    Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, ErrorBody, Frame, Link, Outbound, Reply, Rpc, RpcCx, Subscription,
    Payload, ReplyPath,
};
use ulo_transport::{Classify, Count, Detail, ErrorKind};

const UNARY: &str = "bad_deadline.unary";
const STREAMED: &str = "bad_deadline.streamed";
const EVENT: &str = "bad_deadline.event";
const STALL: &str = "bad_deadline.stall";

/// How long a test waits for the server's answer or settlement, each of which comes as soon as
/// the frame is read.
const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("never")
    }
}

impl std::error::Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// What one server's handlers record: how many ran, the time left to the deadline each run
/// found, `None` for an execution with none, and how many errors the error handler was offered.
#[derive(Clone, Default)]
struct Runs {
    count: Arc<AtomicUsize>,
    left: Arc<Mutex<Vec<Option<Duration>>>>,
    offered: Arc<AtomicUsize>,
}

impl Runs {
    fn ran(&self, exec: &ExecutionRef) {
        let left = exec.deadline().map(|deadline| deadline.saturating_duration_since(Instant::now()));
        self.left.lock().unwrap_or_else(PoisonError::into_inner).push(left);
        self.count.fetch_add(1, Ordering::SeqCst);
    }

    fn count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn left(&self) -> Vec<Option<Duration>> {
        self.left.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn offered(&self) -> usize {
        self.offered.load(Ordering::SeqCst)
    }
}

/// Counts every error it is offered and claims none.
struct Counting(Runs);

impl ErrorHandler<Rpc> for Counting {
    async fn handle(&self, err: BoxError, _cx: &RpcCx) -> Result<Reply, BoxError> {
        self.0.offered.fetch_add(1, Ordering::SeqCst);
        Err(err)
    }
}

#[controller]
struct Handlers {
    runs: Dep<Runs>,
}

#[handlers]
impl Handlers {
    #[ulo_rpc::message("bad_deadline.unary")]
    async fn unary(&self, exec: ExecutionRef) -> Result<u32, Never> {
        self.runs.ran(&exec);
        Ok(1)
    }

    #[ulo_rpc::message("bad_deadline.streamed")]
    async fn streamed(&self, exec: ExecutionRef, items: ulo_rpc::Inbound<u32>) -> Result<u32, Never> {
        self.runs.ran(&exec);
        Ok(items.count().await as u32)
    }

    #[ulo_rpc::event("bad_deadline.event")]
    async fn event(&self, exec: ExecutionRef, _value: Payload<u32>) -> Result<(), Never> {
        self.runs.ran(&exec);
        Ok(())
    }

    /// Waits a minute, so only its deadline ends it within a test.
    #[ulo_rpc::message("bad_deadline.stall")]
    async fn stall(&self, exec: ExecutionRef, timer: Dep<dyn Timer>) -> Result<u32, Never> {
        self.runs.ran(&exec);
        timer.sleep(Duration::from_secs(60)).await;
        Ok(0)
    }
}

struct Root {
    runs: Runs,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.runs.clone());
        m.controller::<Handlers>();
        let counting: Arc<AnyErrorHandler<Rpc>> = Arc::new(Counting(self.runs.clone()));
        m.enhancer::<AnyErrorHandler<Rpc>>().value(counting);
    }
}

/// A link whose inbound stream the test feeds.
struct Scripted(Mutex<Option<Inbound>>);

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).codec(Codec::Json)
    }

    async fn listen(&self, _patterns: &[Subscription]) -> Result<Inbound, BoxError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// What the server did with one delivery: the frames it wrote back, and how it settled the
/// delivery, `Some(true)` acknowledged and `Some(false)` rejected.
#[derive(Debug)]
struct Settled {
    frames: Vec<Frame>,
    ack: Option<bool>,
}

struct Running {
    handle: AppHandle,
    serving: JoinHandle<()>,
    deliveries: mpsc::UnboundedSender<Delivery>,
    runs: Runs,
}

impl Running {
    async fn start() -> Running {
        Running::start_with(Count::Default).await
    }

    async fn start_with(max_inflight: Count) -> Running {
        let (deliveries, mut receiver) = mpsc::unbounded_channel::<Delivery>();
        let inbound: Inbound = Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx)));
        let runs = Runs::default();
        let app = App::builder(Root { runs: runs.clone() })
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_rpc::Server::new(Scripted(Mutex::new(Some(inbound)))).max_inflight(max_inflight))
            .listen()
            .await
            .expect("the server binds");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { handle, serving, deliveries, runs }
    }

    /// Delivers `frame`, a reply path beside it unless it is an event, and waits until the server
    /// has written the call's last frame, an `res`, an `err` or an `end`, and settled it; for an
    /// event, until the server has settled it.
    async fn deliver(&self, frame: Frame) -> Settled {
        let event = matches!(frame, Frame::Evt { .. });
        let (frames, mut written) = mpsc::unbounded_channel();
        let reply = ReplyPath::new(move |frame: Frame| {
            let sent = frames.send(frame).map_err(BoxError::from);
            Box::pin(async move { sent })
        });
        let (settle, mut settled) = mpsc::unbounded_channel::<bool>();
        let ack = Ack::new(move |accepted| {
            let _ = settle.send(accepted);
        });
        let reply = (!event).then_some(reply);
        self.deliveries.send(Delivery { frame, reply, ack }).expect("the server reads its link");
        let waited = tokio::time::timeout(PATIENCE, async {
            let mut frames = Vec::new();
            if !event {
                while let Some(frame) = written.recv().await {
                    let last = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
                    frames.push(frame);
                    if last {
                        break;
                    }
                }
            }
            Settled { frames, ack: settled.recv().await }
        });
        waited.await.unwrap_or_else(|_| panic!("the server neither answered nor settled the delivery within {PATIENCE:?}"))
    }

    async fn stop(self) {
        let Running { handle, serving, deliveries, .. } = self;
        drop(deliveries);
        let _ = handle.close(Signal::new("bad deadline test")).await;
        let _ = serving.await;
    }
}

fn deadline(value: &str) -> CallHeaders {
    let mut headers = CallHeaders::new();
    headers.insert("deadline-ms", value);
    headers
}

fn reason(error: &ErrorBody) -> Option<&str> {
    error.details.iter().find_map(|detail| match detail {
        Detail::ErrorInfo { reason, .. } => Some(reason.as_str()),
        _ => None,
    })
}

/// Asserts the server refused a request or streamed request `bad_deadline`, on its only frame,
/// and acknowledged it.
fn refused(settled: &Settled, id: u64, what: &str) {
    match settled.frames.as_slice() {
        [Frame::Err { id: answered, error, .. }] if *answered == id => {
            assert_eq!((error.kind, reason(error)), (ErrorKind::BadRequest, Some("bad_deadline")), "{what}: the refusal: {error:?}");
        }
        other => panic!("{what}: expected one `err` frame refusing the call `bad_deadline`, got: {other:?}"),
    }
    assert_eq!(settled.ack, Some(true), "{what}: a refused call is answered, so it is acknowledged rather than redelivered");
}

/// A request, a streamed request with its items, and an event, each carrying `value` as its
/// `deadline-ms`: the first two refused `bad_deadline`, the event rejected, and no handler run.
async fn refuses(value: &str) {
    let running = Running::start().await;
    let what = format!("a request whose `deadline-ms` is {value:?}");
    let settled = running.deliver(Frame::req(1, UNARY, deadline(value), Data::new(b"null".as_slice()))).await;
    refused(&settled, 1, &what);

    let what = format!("a streamed request whose `deadline-ms` is {value:?}");
    let opened = running.deliver(Frame::open(2, STREAMED, deadline(value)));
    let (settled, ()) = tokio::join!(opened, async {
        // The items and their end reach a call the server never opened, and are dropped.
        let items = [Frame::input(2, Data::new(b"1".as_slice())), Frame::in_end(2)];
        for frame in items {
            running.deliveries.send(Delivery { frame, reply: None, ack: Ack::none() }).expect("the server reads its link");
        }
    });
    refused(&settled, 2, &what);

    let settled = running.deliver(Frame::evt(EVENT, deadline(value), Data::new(b"7".as_slice()))).await;
    assert_eq!(settled.ack, Some(false), "an event whose `deadline-ms` is {value:?} was not rejected: {settled:?}");

    assert_eq!(running.runs.count(), 0, "handlers that ran for calls whose `deadline-ms` is {value:?}");
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_overflowing_deadline_is_refused_before_the_handler_runs() {
    refuses("18446744073709551615999").await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_deadline_that_is_not_a_number_is_refused_before_the_handler_runs() {
    refuses("abc").await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_negative_deadline_is_refused_before_the_handler_runs() {
    refuses("-5").await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_deadline_is_refused_before_the_handler_runs() {
    refuses("").await;
}

/// The refusal quotes what it cannot read, cut short past 64 characters, so a caller sees the
/// value its own code wrote.
#[tokio::test(flavor = "current_thread")]
async fn the_refusal_quotes_the_value_cut_short() {
    let running = Running::start().await;
    let long = "9".repeat(100);
    for (id, value) in [(1, "abc".to_owned()), (2, long.clone())] {
        let settled = running.deliver(Frame::req(id, UNARY, deadline(&value), Data::new(b"null".as_slice()))).await;
        refused(&settled, id, &format!("a request whose `deadline-ms` is {value:?}"));
        let [Frame::Err { error, .. }] = settled.frames.as_slice() else { unreachable!("`refused` matched one `err`") };
        let quoted: String = value.chars().take(64).collect();
        assert!(error.message.contains(&format!("{quoted:?}")), "the refusal does not quote {quoted:?}: {}", error.message);
        assert_eq!(error.message.contains("cut short"), value.len() > 64, "whether the refusal says it cut the value short: {}", error.message);
    }
    running.stop().await;
}

/// A readable `deadline-ms` still sets the execution's deadline, and a call outliving it is
/// answered `timeout`.
#[tokio::test(flavor = "current_thread")]
async fn a_readable_deadline_still_sets_the_execution_s() {
    let running = Running::start().await;
    let settled = running.deliver(Frame::req(1, UNARY, deadline("5000"), Data::new(b"null".as_slice()))).await;
    assert!(matches!(settled.frames.as_slice(), [Frame::Res { id: 1, .. }]), "a call with a five-second deadline was not answered: {settled:?}");
    let started = Instant::now();
    let settled = running.deliver(Frame::req(2, STALL, deadline("50"), Data::new(b"null".as_slice()))).await;
    assert!(
        matches!(settled.frames.as_slice(), [Frame::Err { id: 2, error, .. }] if error.kind == ErrorKind::Timeout),
        "a call stalling past its 50 ms deadline was not answered `timeout`: {settled:?}"
    );
    assert!(started.elapsed() < Duration::from_secs(5), "the 50 ms deadline took {:?}", started.elapsed());
    let left = running.runs.left();
    assert!(
        matches!(left.as_slice(), [Some(five), Some(fifty)] if *five > Duration::from_secs(4) && *five <= Duration::from_secs(5) && *fifty <= Duration::from_millis(50)),
        "the deadlines the two handlers found: {left:?}"
    );
    running.stop().await;
}

/// A zero `deadline-ms` is a budget already spent: a request and a streamed request are answered
/// `err` of kind `timeout` and acknowledged, and an event rejected, with no handler run and the
/// error handler offered nothing, whatever spaces or leading zeros spell the zero. Planted
/// break: the zero opening its execution as any deadline does, which runs the handler once.
#[tokio::test(flavor = "current_thread")]
async fn a_spent_deadline_is_answered_timeout_with_no_handler_run() {
    let running = Running::start().await;
    for (n, value) in ["0", " 0 ", "000"].into_iter().enumerate() {
        let id = 2 * n as u64 + 1;
        let settled = running.deliver(Frame::req(id, STALL, deadline(value), Data::new(b"null".as_slice()))).await;
        assert!(
            matches!(settled.frames.as_slice(), [Frame::Err { id: answered, error, .. }] if *answered == id && error.kind == ErrorKind::Timeout && reason(error).is_none()),
            "a call whose `deadline-ms` is {value:?} was not answered `timeout`: {settled:?}"
        );
        assert_eq!(settled.ack, Some(true), "a call answered `timeout` at once is acknowledged: {value:?}");

        let opened = running.deliver(Frame::open(id + 1, STREAMED, deadline(value)));
        let (settled, ()) = tokio::join!(opened, async {
            let items = [Frame::input(id + 1, Data::new(b"1".as_slice())), Frame::in_end(id + 1)];
            for frame in items {
                running.deliveries.send(Delivery { frame, reply: None, ack: Ack::none() }).expect("the server reads its link");
            }
        });
        assert!(
            matches!(settled.frames.as_slice(), [Frame::Err { error, .. }] if error.kind == ErrorKind::Timeout),
            "a streamed request whose `deadline-ms` is {value:?} was not answered `timeout`: {settled:?}"
        );

        let settled = running.deliver(Frame::evt(EVENT, deadline(value), Data::new(b"7".as_slice()))).await;
        assert_eq!(settled.ack, Some(false), "an event whose `deadline-ms` is {value:?} was not rejected: {settled:?}");
    }
    assert_eq!(running.runs.count(), 0, "handlers that ran for calls whose budget was spent");
    assert_eq!(running.runs.offered(), 0, "errors the error handler was offered for calls whose budget was spent");
    running.stop().await;
}

/// A spent and an unreadable `deadline-ms` are answered before admission: with the server's one
/// slot held by a call in flight, each is answered its own refusal, `timeout` and `bad_deadline`,
/// not the shed's `unavailable`, which a caller would retry. Planted break: the spent check moved
/// after `try_admit`.
#[tokio::test(flavor = "current_thread")]
async fn a_spent_or_unreadable_deadline_is_answered_before_admission() {
    let running = Running::start_with(Count::Max(1)).await;
    // The held call's replies are never read; the receiver is kept so its sends succeed.
    let (held, _written) = mpsc::unbounded_channel::<Frame>();
    let reply = ReplyPath::new(move |frame: Frame| {
        let sent = held.send(frame).map_err(BoxError::from);
        Box::pin(async move { sent })
    });
    let stall = Frame::req(1, STALL, deadline("60000"), Data::new(b"null".as_slice()));
    running.deliveries.send(Delivery { frame: stall, reply: Some(reply.clone()), ack: Ack::none() }).expect("the server reads its link");
    tokio::time::timeout(PATIENCE, async {
        while running.runs.count() == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the held call's handler ran");

    let settled = running.deliver(Frame::req(2, UNARY, deadline("1"), Data::new(b"null".as_slice()))).await;
    assert!(
        matches!(settled.frames.as_slice(), [Frame::Err { id: 2, error, .. }] if error.kind == ErrorKind::Unavailable),
        "the server's one slot is not held: {settled:?}"
    );
    let settled = running.deliver(Frame::req(3, UNARY, deadline("0"), Data::new(b"null".as_slice()))).await;
    assert!(
        matches!(settled.frames.as_slice(), [Frame::Err { id: 3, error, .. }] if error.kind == ErrorKind::Timeout),
        "a spent deadline at a full server: {settled:?}"
    );
    let settled = running.deliver(Frame::req(4, UNARY, deadline("x"), Data::new(b"null".as_slice()))).await;
    refused(&settled, 4, "an unreadable deadline at a full server");

    running.deliveries.send(Delivery { frame: Frame::cancel(1), reply: Some(reply), ack: Ack::none() }).expect("the server reads its link");
    running.stop().await;
}

/// The smallest budget not yet spent, one millisecond, runs the handler under that deadline, and a
/// handler that waits is ended by it and answered `timeout` through the error handlers.
#[tokio::test(flavor = "current_thread")]
async fn a_one_millisecond_deadline_runs_the_handler() {
    let running = Running::start().await;
    let settled = running.deliver(Frame::req(1, STALL, deadline("1"), Data::new(b"null".as_slice()))).await;
    assert!(
        matches!(settled.frames.as_slice(), [Frame::Err { id: 1, error, .. }] if error.kind == ErrorKind::Timeout),
        "a call whose `deadline-ms` is 1 was not answered `timeout`: {settled:?}"
    );
    assert_eq!(running.runs.count(), 1, "the handler of a call with a millisecond left");
    assert_eq!(running.runs.offered(), 1, "the error handler is offered the `Timeout` of a deadline that passed while the handler ran");
    running.stop().await;
}
