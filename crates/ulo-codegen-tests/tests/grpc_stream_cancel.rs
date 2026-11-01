//! A gRPC reply stream cancelled while it runs follows the pipeline's order: an item ready in the
//! poll the cancellation wakes is written, and no item is taken after that poll, so a stream whose
//! items are always ready ends at its next item. The drain's cancellation ends the stream with
//! UNAVAILABLE and the shutdown phrase in its trailers, a passed deadline with DEADLINE_EXCEEDED.
//!
//! Each stream that is cancelled writes the drain's reason itself, or the test fires the deadline
//! through the app's clock, which fixes the poll the cancellation is observed in. The calls go through a
//! plain HTTP/2 client, which leaves the deadline to the server.

mod support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt, stream};
use tokio::sync::{Notify, mpsc, watch};
use tonic::Code;
use ulo::{App, BoxError, BoxFuture, CancelReason, ErrorHandler, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, StreamOutcome, Timer, injectable, routes};
use ulo_codegen_tests::probe::ends_client::EndsClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_transport::{CallError, ErrorKind};

use support::{PlainHttp2, within};

/// How many items the six-item streams have; each would end after its last.
const SIX: u32 = 6;
/// The item the self-cancelling stream cancels its execution on.
const CANCELLED_AT: u32 = 3;
/// The `grpc-timeout` of the deadline test; the test's clock fires a sleep of exactly this length
/// when the test says, and passes every other sleep to the runtime's clock.
const DEADLINE: Duration = Duration::from_secs(7);

/// What the streams share with their tests.
struct Shared {
    produced: AtomicUsize,
    /// The gated streams' second item waits here, and `waiting` is notified as it starts to.
    gate: Notify,
    waiting: Notify,
    /// The gated call's execution, for the test to cancel.
    exec: Mutex<Option<ExecutionRef>>,
    outcome: Mutex<Option<mpsc::UnboundedSender<StreamOutcome>>>,
}

impl Shared {
    fn new() -> Shared {
        Shared { produced: AtomicUsize::new(0), gate: Notify::new(), waiting: Notify::new(), exec: Mutex::new(None), outcome: Mutex::new(None) }
    }

    /// Registers the callback that reports the call's outcome, owning the channel's one sender, so
    /// the channel closes when the call's execution ends.
    fn report(&self, exec: &ExecutionRef) {
        let sender = self.outcome.lock().unwrap_or_else(PoisonError::into_inner).take().expect("one call per test");
        exec.on_stream_end(move |outcome| {
            let _ = sender.send(outcome);
        });
    }

    /// Six ticks: the first ready, the second once the gate opens, the rest ready.
    fn gated(shared: ulo::Dep<SharedHandle>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        shared.0.report(&exec);
        *shared.0.exec.lock().unwrap_or_else(PoisonError::into_inner) = Some(exec);
        let shared = std::sync::Arc::clone(&shared.0);
        stream::iter(1..=SIX).then(move |n| {
            let shared = std::sync::Arc::clone(&shared);
            async move {
                if n == 2 {
                    shared.waiting.notify_one();
                    shared.gate.notified().await;
                }
                shared.produced.fetch_add(1, Ordering::SeqCst);
                Ok(Tick { n })
            }
        })
    }
}

#[derive(Clone)]
struct SharedHandle(std::sync::Arc<Shared>);

/// Never answers: the error handlers of a failed item are still running when the deadline passes.
struct Hang;

impl ErrorHandler<Grpc> for Hang {
    async fn handle(&self, _err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        std::future::pending().await
    }
}

#[injectable]
struct Streams {
    shared: ulo::Dep<SharedHandle>,
}

#[routes]
impl Streams {
    /// Six ticks, every one ready, the third cancelling the execution with the drain's reason as
    /// it is produced: the item and the cancellation are ready in one poll.
    #[ulo_grpc::method(probe::ends::Written)]
    fn written(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        self.shared.0.report(&exec);
        let shared = std::sync::Arc::clone(&self.shared.0);
        stream::iter(1..=SIX).map(move |n| {
            shared.produced.fetch_add(1, Ordering::SeqCst);
            if n == CANCELLED_AT {
                exec.cancel_with(CancelReason::Drain);
            }
            Ok(Tick { n })
        })
    }

    #[ulo_grpc::method(probe::ends::Abandoned)]
    fn abandoned(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        Shared::gated(self.shared.clone(), exec)
    }

    #[ulo_grpc::method(probe::ends::Expired)]
    fn expired(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        Shared::gated(self.shared.clone(), exec)
    }

    /// One tick, then an `Err` item whose error handler never answers.
    #[ulo_grpc::method(probe::ends::Failed)]
    #[error_handlers(value = Hang)]
    fn failed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        self.shared.0.report(&exec);
        let shared = std::sync::Arc::clone(&self.shared.0);
        stream::iter(1..=2).map(move |n| {
            if n == 2 {
                shared.waiting.notify_one();
                return Err(CallError::new(ErrorKind::Internal, "an item failed"));
            }
            Ok(Tick { n })
        })
    }
}

struct Root(SharedHandle);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Streams>();
    }
}

/// The runtime's clock, with every sleep of [`DEADLINE`] held until the test fires it.
#[derive(Clone)]
struct Clock(watch::Sender<bool>, ulo_runtime_tokio::Timer);

impl Timer for Clock {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        if d != DEADLINE {
            return self.1.sleep(d);
        }
        let mut fired = self.0.subscribe();
        Box::pin(async move {
            let _ = fired.wait_for(|fired| *fired).await;
        })
    }

    fn now(&self) -> Instant {
        self.1.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        let sleep = self.sleep(d);
        Box::pin(async move {
            sleep.await;
            Ok(())
        })
    }
}

struct Running {
    shared: std::sync::Arc<Shared>,
    clock: Clock,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: EndsClient<PlainHttp2>,
    outcome: mpsc::UnboundedReceiver<StreamOutcome>,
}

impl Running {
    async fn start() -> Running {
        let shared = std::sync::Arc::new(Shared::new());
        let (sender, outcome) = mpsc::unbounded_channel();
        *shared.outcome.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        let clock = Clock(watch::channel(false).0, ulo_runtime_tokio::Timer::current());
        let app = App::builder(Root(SharedHandle(std::sync::Arc::clone(&shared))))
            .runtime(ulo_runtime_tokio::Tokio::current())
            .timer(clock.clone())
            .drain_timeout(Duration::from_secs(4))
            .wire()
            .unwrap_or_else(|error| panic!("the gRPC app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the gRPC app did not connect: {error}"))
            .bind(ulo_grpc::Server::new("127.0.0.1:0"))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the gRPC app did not listen: {error}"));
        let addr = match app.addresses().as_slice() {
            [bound] => bound.addr,
            other => panic!("expected one bound address, got {other:?}"),
        };
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        let http2 = hyper_util::client::legacy::Client::builder(hyper_util::rt::TokioExecutor::new()).http2_only(true).build_http::<tonic::body::Body>();
        let origin = format!("http://{addr}").parse().unwrap_or_else(|error| panic!("bad origin: {error}"));
        Running { shared, clock, handle, serving, client: EndsClient::with_origin(http2, origin), outcome }
    }

    /// Waits for the gated stream to wait at its gate, then readies both the item and `cancel` before
    /// the call's next poll: one thread runs the server and the test.
    async fn cancel_at_the_gate(&self, cancel: impl FnOnce(&ExecutionRef)) {
        within("the stream waiting at its gate", self.shared.waiting.notified()).await;
        let exec = self.shared.exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
        cancel(&exec);
        self.shared.gate.notify_one();
    }

    /// The ticks after `first`, and the status the reply ended with: `Ok` for `grpc-status: 0`.
    async fn rest(items: &mut tonic::Streaming<Tick>, mut ticks: Vec<u32>) -> (Vec<u32>, Code, String) {
        loop {
            match within("the next item", items.next()).await {
                Some(Ok(tick)) => ticks.push(tick.n),
                Some(Err(status)) => return (ticks, status.code(), status.message().to_owned()),
                None => return (ticks, Code::Ok, String::new()),
            }
        }
    }

    /// What the call's execution reported once it ended.
    async fn ended(&mut self) -> Option<StreamOutcome> {
        let first = within("the call's execution ending", self.outcome.recv()).await;
        if first.is_some() {
            assert_eq!(within("the outcome channel closing", self.outcome.recv()).await, None, "the execution reported its stream's end twice");
        }
        first
    }

    async fn stop(self) {
        drop(self.client);
        within("the app's close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

const SHUTTING_DOWN: &str = ulo_transport::SHUTDOWN_MESSAGE;
const DEADLINE_PASSED: &str = "the call's deadline passed";

#[tokio::test]
async fn a_stream_whose_items_are_always_ready_takes_no_item_past_the_poll_its_cancellation_is_observed_in() {
    let mut app = Running::start().await;
    let reply = within("the call", app.client.written(Ticks::default())).await.expect("the call is answered");
    let mut items = reply.into_inner();

    let (ticks, code, message) = Running::rest(&mut items, Vec::new()).await;
    assert_eq!(ticks, (1..=CANCELLED_AT).collect::<Vec<_>>(), "the items written before the trailers");
    assert_eq!((code, message.as_str()), (Code::Unavailable, SHUTTING_DOWN), "the trailers that end the stream");
    assert_eq!(app.shared.produced.load(Ordering::SeqCst), CANCELLED_AT as usize, "an item was taken after the cancellation");
    drop(items);
    assert_eq!(app.ended().await, Some(StreamOutcome::CutOff(Some(CancelReason::Drain))));
    app.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_item_ready_in_the_poll_the_cancellation_wakes_is_written_and_none_after_it() {
    let mut app = Running::start().await;
    let reply = within("the call", app.client.abandoned(Ticks::default())).await.expect("the call is answered");
    let mut items = reply.into_inner();
    let first = within("the first item", items.next()).await.expect("an item").expect("no error yet");
    app.cancel_at_the_gate(|exec| exec.cancel_with(CancelReason::Drain)).await;

    let (ticks, code, message) = Running::rest(&mut items, vec![first.n]).await;
    assert_eq!(ticks, [1, 2], "the items written before the trailers");
    assert_eq!((code, message.as_str()), (Code::Unavailable, SHUTTING_DOWN), "the trailers that end the stream");
    assert_eq!(app.shared.produced.load(Ordering::SeqCst), 2, "an item was produced after the poll the cancellation woke");
    drop(items);
    assert_eq!(app.ended().await, Some(StreamOutcome::CutOff(Some(CancelReason::Drain))));
    app.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_item_ready_in_the_poll_the_deadline_passes_in_is_written_and_none_after_it() {
    let mut app = Running::start().await;
    let mut request = tonic::Request::new(Ticks::default());
    request.set_timeout(DEADLINE);
    let reply = within("the call", app.client.expired(request)).await.expect("the call is answered");
    let mut items = reply.into_inner();
    let first = within("the first item", items.next()).await.expect("an item").expect("no error yet");
    let clock = app.clock.clone();
    app.cancel_at_the_gate(|_| {
        clock.0.send_replace(true);
    })
    .await;

    let (ticks, code, message) = Running::rest(&mut items, vec![first.n]).await;
    assert_eq!(ticks, [1, 2], "the items written before the trailers");
    assert_eq!((code, message.as_str()), (Code::DeadlineExceeded, DEADLINE_PASSED), "the trailers that end the stream");
    assert_eq!(app.shared.produced.load(Ordering::SeqCst), 2, "an item was produced after the poll the deadline passed in");
    drop(items);
    assert_eq!(app.ended().await, Some(StreamOutcome::CutOff(Some(CancelReason::Deadline))));
    app.stop().await;
}

/// The deadline passing while the error handlers of a failed item still run ends the reply: the
/// body does not wait for them.
#[tokio::test(flavor = "current_thread")]
async fn a_deadline_passing_while_a_failed_items_error_handlers_run_ends_the_stream() {
    let mut app = Running::start().await;
    let mut request = tonic::Request::new(Ticks::default());
    request.set_timeout(DEADLINE);
    let reply = within("the call", app.client.failed(request)).await.expect("the call is answered");
    let mut items = reply.into_inner();
    let first = within("the first item", items.next()).await.expect("an item").expect("no error yet");
    within("the failing item", app.shared.waiting.notified()).await;
    app.clock.0.send_replace(true);

    let (ticks, code, message) = Running::rest(&mut items, vec![first.n]).await;
    assert_eq!(ticks, [1], "the items written before the trailers");
    assert_eq!((code, message.as_str()), (Code::DeadlineExceeded, DEADLINE_PASSED), "the trailers that end the stream");
    drop(items);
    assert_eq!(app.ended().await, Some(StreamOutcome::CutOff(Some(CancelReason::Deadline))));
    app.stop().await;
}
