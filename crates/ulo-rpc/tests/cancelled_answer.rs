//! What the dispatcher writes once a caller cancels its call, over a link the test drives. The
//! handler is polled before the cancellation, so an answer it chooses on observing the cancel is
//! written on the reply path: the caller that cancelled discards it. A reply stream follows the
//! same order: an item ready in the poll the cancellation wakes is written, and no item is taken
//! after that poll, so a stream whose items are always ready ends at its next item, at a
//! cancellation and at a passed deadline alike.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_core::Stream;
use futures_util::{StreamExt, stream};
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use ulo::{App, AppHandle, BoxError, BoxFuture, CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, injectable, routes};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, Frame, Link, Outbound, Pattern, ReplyPath};
use ulo_transport::{CallError, Classify, ErrorKind};

const OBSERVE: &str = "cancelled.observe";
const ENDLESS: &str = "cancelled.endless";
const SELF_DRAINED: &str = "cancelled.self_drained";
const GATED: &str = "cancelled.gated";

/// How many items the six-item streams below have; each would end after its last.
const ITEMS: usize = 6;
/// The item `self_drained` cancels its execution on.
const CANCELLED_AT: usize = 3;

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(5);

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

/// What the handlers tell the test.
#[derive(Default)]
struct Probe {
    entered: Notify,
    /// Set when the endless stream drops.
    stream_dropped: Arc<AtomicBool>,
    /// How many items the six-item stream of the call under test produced.
    produced: Arc<AtomicUsize>,
    /// Set when the six-item stream of the call under test drops.
    six_dropped: Arc<AtomicBool>,
    /// The gated stream's second item waits here.
    gate: Arc<Notify>,
    /// Notified when the gated stream starts waiting for its second item.
    waiting: Arc<Notify>,
    /// The gated call's execution, for the test to cancel.
    exec: Mutex<Option<ExecutionRef>>,
}

#[derive(Clone)]
struct Shared(Arc<Probe>);

struct OnDrop(Arc<AtomicBool>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[injectable]
struct Handlers {
    probe: Dep<Shared>,
}

#[routes]
impl Handlers {
    /// Waits for its execution's cancellation, then answers with the reason it read.
    #[ulo_rpc::message("cancelled.observe")]
    async fn observe(&self, exec: ExecutionRef) -> Result<String, Never> {
        self.probe.0.entered.notify_one();
        exec.cancelled().await;
        Ok(format!("observed {:?}", exec.cancel_reason()))
    }

    /// Six items, every one ready, the third cancelling its own execution with the drain's
    /// reason as it is produced: the item and the cancellation are ready in one poll.
    #[ulo_rpc::message("cancelled.self_drained")]
    async fn self_drained(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, CallError>> {
        let probe = Arc::clone(&self.probe.0);
        let dropped = OnDrop(Arc::clone(&probe.six_dropped));
        stream::iter(1..=ITEMS as u64).map(move |n| {
            let _ = &dropped;
            probe.produced.fetch_add(1, Ordering::SeqCst);
            if n == CANCELLED_AT as u64 {
                exec.cancel_with(CancelReason::Drain);
            }
            Ok(n)
        })
    }

    /// Six items: the first ready, the second once the gate opens, the rest ready.
    #[ulo_rpc::message("cancelled.gated")]
    async fn gated(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u64, CallError>> {
        *lock(&self.probe.0.exec) = Some(exec);
        let probe = Arc::clone(&self.probe.0);
        let dropped = OnDrop(Arc::clone(&probe.six_dropped));
        stream::iter(1..=ITEMS as u64).then(move |n| {
            let (probe, _) = (Arc::clone(&probe), &dropped);
            async move {
                if n == 2 {
                    probe.waiting.notify_one();
                    probe.gate.notified().await;
                }
                probe.produced.fetch_add(1, Ordering::SeqCst);
                Ok(n)
            }
        })
    }

    /// A stream whose every item is ready.
    #[ulo_rpc::message("cancelled.endless")]
    async fn endless(&self) -> impl Stream<Item = Result<u64, CallError>> {
        let dropped = OnDrop(Arc::clone(&self.probe.0.stream_dropped));
        stream::repeat(7).map(move |n| {
            let _ = &dropped;
            Ok(n)
        })
    }
}

struct Root(Shared);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Handlers>();
    }
}

/// A link whose inbound stream the test feeds.
struct Scripted(Mutex<Option<Inbound>>);

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).miss_signal(true)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        lock(&self.0).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

struct Running {
    probe: Shared,
    handle: AppHandle,
    serving: JoinHandle<()>,
    deliveries: mpsc::UnboundedSender<Delivery>,
}

impl Running {
    async fn start() -> Running {
        let (deliveries, mut receiver) = mpsc::unbounded_channel::<Delivery>();
        let inbound: Inbound = Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx)));
        let probe = Shared(Arc::new(Probe::default()));
        let app = App::builder(Root(probe.clone()))
            .runtime(ulo_runtime_tokio::Tokio::current())
            .drain_timeout(Duration::from_millis(100))
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_rpc::Server::new(Scripted(Mutex::new(Some(inbound)))))
            .listen()
            .await
            .expect("the server binds");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { probe, handle, serving, deliveries }
    }

    fn deliver(&self, frame: Frame, reply: Option<ReplyPath>) {
        self.deliveries.send(Delivery { frame, reply, ack: Ack::none() }).expect("the server reads its inbound stream");
    }

    async fn stop(self) {
        let _ = within("close", self.handle.close(Signal::new("cancelled answer test"))).await;
        let _ = within("serve", self.serving).await;
    }
}

/// A reply path whose frames `frames` receives, each send yielding once first, so a call writing
/// frames that are always ready still lets the test's task run between them.
fn replies() -> (ReplyPath, mpsc::UnboundedReceiver<Frame>) {
    let (frames, received) = mpsc::unbounded_channel();
    let path = ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let frames = frames.clone();
        Box::pin(async move {
            tokio::task::yield_now().await;
            frames.send(frame)?;
            Ok(())
        })
    });
    (path, received)
}

fn request(id: u64, pattern: &str) -> Frame {
    Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) }
}

#[tokio::test(flavor = "current_thread")]
async fn a_handler_answering_once_its_caller_cancels_is_answered_on_the_reply_path() {
    let running = Running::start().await;
    let (path, mut frames) = replies();
    running.deliver(request(1, OBSERVE), Some(path.clone()));
    within("the handler entering", running.probe.0.entered.notified()).await;
    running.deliver(Frame::Cancel { id: 1 }, Some(path));

    let frame = within("the answer", frames.recv()).await.expect("a frame on the reply path");
    match frame {
        Frame::Res { id, data } => {
            assert_eq!(id, 1, "the answer names the cancelled call");
            let answer: String = serde_json::from_slice(data.as_bytes()).expect("a JSON string");
            assert_eq!(answer, format!("observed {:?}", Some(CancelReason::ClientCancelled)));
        }
        other => panic!("the handler's answer was not written: {other:?}"),
    }
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_stream_whose_items_are_always_ready_ends_at_its_next_item_once_cancelled() {
    let running = Running::start().await;
    let (path, mut frames) = replies();
    running.deliver(request(1, ENDLESS), Some(path.clone()));
    let first = within("the first item", frames.recv()).await.expect("a frame on the reply path");
    assert!(matches!(&first, Frame::Item { id: 1, data } if data.as_bytes() == b"7"), "{first:?}");
    running.deliver(Frame::Cancel { id: 1 }, Some(path));

    within("the stream's drop", async {
        while !running.probe.0.stream_dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let mut after = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        after.push(frame);
    }
    assert!(after.len() <= 2, "the stream ran on after its caller cancelled: {} frames", after.len());
    assert!(after.iter().all(|frame| matches!(frame, Frame::Item { id: 1, .. })), "a cancelled caller is written no end: {after:?}");
    running.stop().await;
}

/// The frames a call wrote once its stream dropped: every frame sent before the drop, since the
/// reply path's sends complete before the stream drops.
async fn written_until(dropped: &AtomicBool, frames: &mut mpsc::UnboundedReceiver<Frame>) -> Vec<Frame> {
    within("the stream's drop", async {
        while !dropped.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let mut written = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        written.push(frame);
    }
    written
}

/// The items `1..=n` under call 1, then the shutdown refusal.
fn items_then_shutdown(written: &[Frame], n: u64) {
    let (last, items) = written.split_last().unwrap_or_else(|| panic!("nothing was written"));
    let numbers: Vec<Option<u64>> = items
        .iter()
        .map(|frame| match frame {
            Frame::Item { id: 1, data } => serde_json::from_slice(data.as_bytes()).ok(),
            _ => None,
        })
        .collect();
    assert_eq!(numbers, (1..=n).map(Some).collect::<Vec<_>>(), "the items written before the stream ended: {written:?}");
    match last {
        Frame::Err { id, error } => {
            assert_eq!(*id, 1, "the refusal names the call");
            assert_eq!((error.kind, error.message.as_str()), (ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE), "{error:?}");
        }
        other => panic!("the stream did not end with the shutdown refusal: {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_always_ready_stream_takes_no_item_past_the_poll_its_cancellation_is_observed_in() {
    let running = Running::start().await;
    let (path, mut frames) = replies();
    running.deliver(request(1, SELF_DRAINED), Some(path));

    let written = written_until(&running.probe.0.six_dropped, &mut frames).await;
    items_then_shutdown(&written, CANCELLED_AT as u64);
    assert_eq!(running.probe.0.produced.load(Ordering::SeqCst), CANCELLED_AT, "an item was taken after the cancellation");
    running.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_item_ready_in_the_poll_the_cancellation_wakes_is_written_and_none_after_it() {
    let running = Running::start().await;
    let (path, mut frames) = replies();
    running.deliver(request(1, GATED), Some(path));
    let first = within("the first item", frames.recv()).await.expect("a frame on the reply path");
    assert!(matches!(&first, Frame::Item { id: 1, data } if data.as_bytes() == b"1"), "{first:?}");
    within("the stream waiting at its gate", running.probe.0.waiting.notified()).await;
    // One task on one thread: the call's next poll finds both the cancellation and the item.
    let exec = lock(&running.probe.0.exec).clone().expect("the handler kept its execution");
    exec.cancel_with(CancelReason::Drain);
    running.probe.0.gate.notify_one();

    let mut written = vec![first];
    written.extend(written_until(&running.probe.0.six_dropped, &mut frames).await);
    items_then_shutdown(&written, 2);
    assert_eq!(running.probe.0.produced.load(Ordering::SeqCst), 2, "an item was taken after the poll the cancellation woke");
    running.stop().await;
}

/// A deadline passing while a stream's items are always ready ends it: the race polls the deadline
/// only while the stream waits, and this stream never does.
#[tokio::test(flavor = "current_thread")]
async fn an_always_ready_stream_ends_at_its_next_item_once_its_deadline_passes() {
    let running = Running::start().await;
    let (path, mut frames) = replies();
    let mut headers = CallHeaders::new();
    headers.insert("deadline-ms", "50");
    running.deliver(Frame::Req { id: 1, pattern: ENDLESS.to_owned(), headers, data: Data::new(b"null".as_slice()) }, Some(path));

    let written = written_until(&running.probe.0.stream_dropped, &mut frames).await;
    let (last, items) = written.split_last().unwrap_or_else(|| panic!("nothing was written"));
    assert!(items.iter().all(|frame| matches!(frame, Frame::Item { id: 1, data } if data.as_bytes() == b"7")), "{items:?}");
    match last {
        Frame::Err { id, error } => {
            assert_eq!(*id, 1, "the refusal names the call");
            assert_eq!((error.kind, error.message.as_str()), (ErrorKind::Timeout, "the call's deadline passed"), "{error:?}");
        }
        other => panic!("the stream did not end with the deadline's error: {other:?}"),
    }
    running.stop().await;
}

async fn within<F: IntoFuture>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not finish within {PATIENCE:?}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
