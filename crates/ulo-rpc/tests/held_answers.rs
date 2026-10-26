//! Answers written from tasks that run no handler, over a link the test drives: the refusal of a
//! call that arrives during the drain or over the in-flight limit, and the answer to a pattern
//! nothing handles. Each reply is sent through a path that holds its send on a gate.
//!
//! The drain waits for such an answer while its send is held: the refusal through the count the
//! drain reads, the unhandled pattern's through the execution it opens. Those tests run on a
//! paused tokio clock beside an app clock that never fires, so a `sleep` of the test's returns
//! only once every task is waiting and the drain window cannot end on its own; a `close` still
//! running then is held by the gate and nothing else.
//!
//! Under a `shutdown_timeout` already spent, `close` is polled once, and its first poll polls each
//! of the server's tasks: a send whose gate opened without waking it completes there. Those tests
//! run on tokio's own clock, which the cap's zero-length sleep needs.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::stream;
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinHandle;
use ulo::{App, AppHandle, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Runtime, Shutdown, ShutdownError, Signal, Timer, injectable, routes};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, ErrorBody, Frame, Link, Outbound, Pattern, Payload, ReplyPath};
use ulo_transport::{Classify, Count, Detail, Details, ErrorKind};

const HELD: &str = "held.wait";
const NOWHERE: &str = "held.nowhere";

/// The longest a test waits on tokio's own clock for something that must happen.
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

/// Tells the test a call reached [`Waiter::wait`].
#[derive(Clone, Default)]
struct Entered(Arc<Notify>);

#[injectable]
struct Waiter {
    entered: ulo::Dep<Entered>,
}

#[routes]
impl Waiter {
    /// Waits for nothing that ever comes, so only its execution's cancellation ends it.
    #[ulo_rpc::message("held.wait")]
    async fn wait(&self, _n: Payload<u32>) -> Result<u32, Never> {
        self.entered.0.notify_one();
        std::future::pending().await
    }
}

struct Root(Entered);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Waiter>();
    }
}

/// What the test sees of its link and does through it.
#[derive(Default)]
struct Script {
    inbound: Mutex<Option<Inbound>>,
    /// The inbound stream's sender; `None` once the test ends the stream.
    sender: Mutex<Option<mpsc::UnboundedSender<Delivery>>>,
    drained: watch::Sender<bool>,
}

/// A link whose inbound stream the test feeds and ends, and whose `drain` and `close` return at
/// once.
struct Scripted(Arc<Script>);

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).miss_signal(true)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        lock(&self.0.inbound).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {
        self.0.drained.send_replace(true);
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// The frames the server sent, in the order their sends completed.
#[derive(Clone, Default)]
struct Sent(Arc<Mutex<Vec<Frame>>>);

impl Sent {
    fn frames(&self) -> Vec<Frame> {
        lock(&self.0).clone()
    }

    fn of(&self, id: u64) -> Vec<Frame> {
        self.frames().into_iter().filter(|frame| frame.id() == Some(id)).collect()
    }
}

/// How a reply path holds its sends.
#[derive(Clone)]
enum Hold {
    /// Sent at once.
    Open,
    /// Held until the gate opens, which wakes the send.
    Gate(watch::Receiver<bool>),
    /// Held until the flag is set, which wakes nothing: only a poll the send did not ask for
    /// finds it set.
    Unwoken(Arc<AtomicBool>),
}

/// A reply path recording each frame into `sent` once `hold` lets its send through, and notifying
/// `entered` as a send first waits.
fn reply(sent: Sent, hold: Hold, entered: Arc<Notify>) -> ReplyPath {
    ReplyPath::new(move |frame: Frame| -> BoxFuture<'static, Result<(), BoxError>> {
        let (sent, hold, entered) = (sent.clone(), hold.clone(), Arc::clone(&entered));
        Box::pin(async move {
            match hold {
                Hold::Open => {}
                Hold::Gate(mut gate) => {
                    if !*gate.borrow() {
                        entered.notify_one();
                    }
                    gate.wait_for(|open| *open).await?;
                }
                Hold::Unwoken(flag) => Unwoken { flag, entered, waited: false }.await,
            }
            lock(&sent.0).push(frame);
            Ok(())
        })
    })
}

/// Pending until its flag is set, registering no waker.
struct Unwoken {
    flag: Arc<AtomicBool>,
    entered: Arc<Notify>,
    waited: bool,
}

impl Future for Unwoken {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        if self.flag.load(Ordering::SeqCst) {
            return Poll::Ready(());
        }
        if !self.waited {
            self.waited = true;
            self.entered.notify_one();
        }
        Poll::Pending
    }
}

fn request(id: u64, pattern: &str, reply: ReplyPath) -> Delivery {
    let frame = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
    Delivery { frame, reply: Some(reply), ack: Ack::none() }
}

/// An app clock that never fires, so no drain window or bound of the app ends while a test runs.
struct Frozen;

impl Timer for Frozen {
    fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(std::future::pending())
    }

    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// One server over the scripted link, serving until closed.
struct Running {
    handle: AppHandle,
    serving: JoinHandle<()>,
    script: Arc<Script>,
    entered: Entered,
}

/// The app's clock and cap for one test.
enum Clock {
    /// [`Frozen`]: the drain window never ends.
    Frozen,
    /// tokio's own clock, under `shutdown_timeout(Duration::ZERO)`.
    SpentCap,
}

impl Running {
    async fn start(clock: Clock, max_inflight: Count) -> Running {
        let (sender, mut receiver) = mpsc::unbounded_channel::<Delivery>();
        let script = Arc::new(Script::default());
        *lock(&script.inbound) = Some(Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx))));
        *lock(&script.sender) = Some(sender);
        let entered = Entered::default();
        let builder = App::builder(Root(entered.clone())).runtime(ulo_runtime_tokio::Tokio::current()).drain_timeout(Duration::from_secs(30));
        let builder = match clock {
            Clock::Frozen => builder.timer(Frozen),
            Clock::SpentCap => builder.shutdown_timeout(Duration::ZERO),
        };
        let app = builder
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_rpc::Server::new(Scripted(Arc::clone(&script))).max_inflight(max_inflight))
            .listen()
            .await
            .expect("the server binds");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { handle, serving, script, entered }
    }

    fn deliver(&self, delivery: Delivery) {
        if let Some(sender) = lock(&self.script.sender).as_ref() {
            let _ = sender.send(delivery);
        }
    }

    /// Ends the link's inbound stream, as a link does once nothing more will arrive.
    fn end_inbound(&self) {
        lock(&self.script.sender).take();
    }

    /// Starts the app's close and waits until the server has called the link's `drain`.
    async fn begin_close(&self) -> JoinHandle<Result<Shutdown, ShutdownError>> {
        let handle = self.handle.clone();
        let closing = tokio::spawn(async move { handle.close(Signal::new("held answers")).await });
        let _ = self.script.drained.subscribe().wait_for(|drained| *drained).await;
        closing
    }
}

/// Returns once every task on the test's runtime is waiting: on a paused clock tokio moves time
/// on only then, and nothing else here sleeps on it.
async fn idle() {
    tokio::time::sleep(Duration::from_secs(3600)).await;
}

fn unavailable(id: u64, message: &str, details: Details) -> Frame {
    Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, message, details) }
}

fn unhandled(id: u64) -> Frame {
    let reason = Detail::ErrorInfo { reason: "pattern_unhandled".to_owned(), domain: "ulo.rpc".to_owned(), metadata: Default::default() };
    unavailable(id, &format!("no handler for pattern `{NOWHERE}`"), Details::new().with(reason))
}

fn shutting_down(id: u64) -> Frame {
    unavailable(id, ulo_transport::SHUTDOWN_MESSAGE, Details::new())
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_refusal_whose_send_is_held_keeps_the_drain_open_until_it_is_sent() {
    let running = Running::start(Clock::Frozen, Count::Default).await;
    let (sent, entered) = (Sent::default(), Arc::new(Notify::new()));
    let (gate, opened) = watch::channel(false);

    let closing = running.begin_close().await;
    running.deliver(request(1, HELD, reply(sent.clone(), Hold::Gate(opened), Arc::clone(&entered))));
    entered.notified().await;
    running.end_inbound();
    idle().await;
    assert!(!closing.is_finished(), "close returned while the refusal's send was held");
    assert!(sent.frames().is_empty(), "a frame was sent through a closed gate: {:?}", sent.frames());

    gate.send_replace(true);
    let shutdown = closing.await.expect("the close task").unwrap_or_else(|error| panic!("the close failed: {error}"));
    assert_eq!(shutdown.abandoned, 0, "the refusal opened no execution to abandon");
    assert_eq!(sent.frames(), vec![shutting_down(1)], "the refusal of the call that arrived during the drain");
    running.serving.await.expect("the serving task");
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn an_unhandled_patterns_answer_whose_send_is_held_keeps_the_drain_open_until_it_is_sent() {
    let running = Running::start(Clock::Frozen, Count::Default).await;
    let (sent, entered) = (Sent::default(), Arc::new(Notify::new()));
    let (gate, opened) = watch::channel(false);

    running.deliver(request(1, NOWHERE, reply(sent.clone(), Hold::Gate(opened), Arc::clone(&entered))));
    entered.notified().await;
    let closing = running.begin_close().await;
    running.end_inbound();
    idle().await;
    assert!(!closing.is_finished(), "close returned while the unhandled pattern's answer was held");
    assert!(sent.frames().is_empty(), "a frame was sent through a closed gate: {:?}", sent.frames());

    gate.send_replace(true);
    let shutdown = closing.await.expect("the close task").unwrap_or_else(|error| panic!("the close failed: {error}"));
    assert_eq!(shutdown.abandoned, 0, "the unhandled pattern's execution ended inside the drain window");
    assert_eq!(sent.frames(), vec![unhandled(1)], "the answer to a pattern nothing handles");
    running.serving.await.expect("the serving task");
}

#[tokio::test(flavor = "current_thread")]
async fn under_a_spent_cap_a_refusal_ready_unwoken_is_sent_in_closes_first_poll() {
    let running = Running::start(Clock::SpentCap, Count::Max(1)).await;
    let (sent, entered) = (Sent::default(), Arc::new(Notify::new()));
    let flag = Arc::new(AtomicBool::new(false));

    running.deliver(request(1, HELD, reply(sent.clone(), Hold::Open, Arc::clone(&entered))));
    tokio::time::timeout(PATIENCE, running.entered.0.notified()).await.expect("the held call reached its handler");
    running.deliver(request(2, HELD, reply(sent.clone(), Hold::Unwoken(Arc::clone(&flag)), Arc::clone(&entered))));
    tokio::time::timeout(PATIENCE, entered.notified()).await.expect("the refusal's send began");
    flag.store(true, Ordering::SeqCst);

    let closed = tokio::time::timeout(PATIENCE, running.handle.close(Signal::new("held answers"))).await.expect("the capped close");
    let shutdown = closed.unwrap_or_else(|error| panic!("a close the scripted link ends in its first poll failed: {error}"));
    assert_eq!(shutdown.abandoned, 1, "the held call is the one execution abandoned");
    let retry = Details::new().with(Detail::RetryAfter(Duration::from_secs(1)));
    assert_eq!(sent.of(2), vec![unavailable(2, "the server is over its in-flight limit", retry)], "the refusal over the in-flight limit");
    assert_eq!(sent.of(1), vec![shutting_down(1)], "the held call abandoned at the drain's end");
    tokio::time::timeout(PATIENCE, running.serving).await.expect("serve returned").expect("the serving task");
}

#[tokio::test(flavor = "current_thread")]
async fn under_a_spent_cap_an_unhandled_patterns_answer_ready_unwoken_is_sent_in_closes_first_poll() {
    let running = Running::start(Clock::SpentCap, Count::Default).await;
    let (sent, entered) = (Sent::default(), Arc::new(Notify::new()));
    let flag = Arc::new(AtomicBool::new(false));

    running.deliver(request(1, NOWHERE, reply(sent.clone(), Hold::Unwoken(Arc::clone(&flag)), Arc::clone(&entered))));
    tokio::time::timeout(PATIENCE, entered.notified()).await.expect("the answer's send began");
    flag.store(true, Ordering::SeqCst);

    let closed = tokio::time::timeout(PATIENCE, running.handle.close(Signal::new("held answers"))).await.expect("the capped close");
    let shutdown = closed.unwrap_or_else(|error| panic!("a close the scripted link ends in its first poll failed: {error}"));
    assert_eq!(shutdown.abandoned, 1, "the unhandled pattern's execution, live at the drain's end");
    assert_eq!(sent.frames(), vec![unhandled(1)], "the answer to a pattern nothing handles");
    tokio::time::timeout(PATIENCE, running.serving).await.expect("serve returned").expect("the serving task");
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
