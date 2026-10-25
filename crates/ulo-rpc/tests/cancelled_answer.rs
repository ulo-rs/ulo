//! What the dispatcher writes once a caller cancels its call, over a link the test drives. The
//! handler is polled before the cancellation, so an answer it chooses on observing the cancel is
//! written on the reply path: the caller that cancelled discards it. A stream whose items are
//! always ready is cut at its next item, not run on.

use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
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

async fn within<F: IntoFuture>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not finish within {PATIENCE:?}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
