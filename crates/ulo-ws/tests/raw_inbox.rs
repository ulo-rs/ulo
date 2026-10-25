//! A hand-written gateway's message in flight at shutdown, over the table and an in-memory pipe,
//! its `on_message` held on a gate.
//!
//! The drain waits for the message: the connection closes with 1001 once `on_message` has
//! returned, after what it sent. That test runs on a paused tokio clock beside an app clock that
//! never fires, so a `sleep` of the test's returns only once every task is waiting and the drain
//! window cannot end on its own; a `close` still running then is held by the gate and nothing
//! else.
//!
//! Under a `shutdown_timeout` already spent, `close` is polled once, and its first poll polls the
//! connection's tasks, the gateway's inbox before the connection: an `on_message` whose gate
//! opened without waking it returns there, and the connection writes what it sent and the 1001
//! close before it is aborted.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use http::request::Parts;
use http::{Method, Version};
use tokio::sync::{Notify, watch};
use tokio::task::JoinHandle;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use ulo::{
    App, AppHandle, BoxError, BoxFuture, Controller, Dep, DrainToken, FailureReason, Limit, Module, ModuleDef, ModuleIdentity, Mount,
    Mounted, Shutdown, ShutdownError, ShutdownFailure, Signal, Timer, TypeName, injectable,
};
use ulo_http::Upgraded;
use ulo_transport::prepare::Failures;
use ulo_ws::{Connection, Frame, Gateway, GatewayConfig, GatewayDefaults, GatewaySettings, GatewayTable, Handshake, Port, Ws};

/// The longest a test waits on tokio's own clock for something that must happen.
const PATIENCE: Duration = Duration::from_secs(5);

const SHUTDOWN_PHRASE: &str = "the server is shutting down";

/// What `on_message` waits on and tells the test.
#[derive(Clone)]
struct Gates {
    /// `/gated` waits for it to open, which wakes it.
    gate: watch::Receiver<bool>,
    /// `/unwoken` waits for it to be set, which wakes nothing.
    flag: Arc<AtomicBool>,
    entered: Arc<Notify>,
}

/// Answers each message with `answered` once its gate opens.
#[injectable]
struct Gated {
    gates: Dep<Gates>,
}

impl GatewayConfig for Gated {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/gated").port(Port::Own)
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

impl Gateway for Gated {
    async fn on_message(&self, conn: &Connection, _frame: Frame) {
        self.gates.entered.notify_one();
        let mut gate = self.gates.gate.clone();
        let _ = gate.wait_for(|open| *open).await;
        let _ = conn.send(Frame::text("answered")).await;
    }
}

impl Controller for Gated {
    fn mount(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

/// Answers each message with `answered` once its flag is set, polled by nothing but a poll it did
/// not ask for.
#[injectable]
struct Unwoken {
    gates: Dep<Gates>,
}

impl GatewayConfig for Unwoken {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/unwoken").port(Port::Own)
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

impl Gateway for Unwoken {
    async fn on_message(&self, conn: &Connection, _frame: Frame) {
        Flagged { flag: Arc::clone(&self.gates.flag), entered: Arc::clone(&self.gates.entered), waited: false }.await;
        let _ = conn.send(Frame::text("answered")).await;
    }
}

impl Controller for Unwoken {
    fn mount(m: &mut Mount<'_>) {
        <Self as Gateway>::mount(m);
    }
}

/// Pending until its flag is set, registering no waker.
struct Flagged {
    flag: Arc<AtomicBool>,
    entered: Arc<Notify>,
    waited: bool,
}

impl Future for Flagged {
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

struct Root(Gates);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Gated>();
        m.controller::<Unwoken>();
    }
}

/// A server with no socket: the table it builds in `prepare` is handed to the test.
struct TableServer {
    table: Arc<Mutex<Option<GatewayTable>>>,
}

impl TableServer {
    fn table(&self) -> Option<GatewayTable> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

impl ulo::Server for TableServer {
    type Transport = Ws;

    async fn prepare(&mut self, mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        let mut failures = Failures::new();
        let table = GatewayTable::own_port("TableServer", &mounted, &GatewayDefaults::default(), &mut failures);
        failures.into_result()?;
        *self.table.lock().unwrap_or_else(PoisonError::into_inner) = Some(table);
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Ws>) -> Result<(), BoxError> {
        if let Some(table) = self.table() {
            table.start();
        }
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, token: DrainToken) {
        if let Some(table) = self.table() {
            table.drain(token).await;
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        if let Some(table) = self.table() {
            table.close().await;
        }
        Ok(())
    }
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

/// The app's clock and cap for one test.
enum Clock {
    /// [`Frozen`]: the drain window never ends.
    Frozen,
    /// tokio's own clock, under `shutdown_timeout(Duration::ZERO)`.
    SpentCap,
}

struct Running {
    table: GatewayTable,
    handle: AppHandle,
    serving: JoinHandle<()>,
    gate: watch::Sender<bool>,
    flag: Arc<AtomicBool>,
    entered: Arc<Notify>,
}

impl Running {
    async fn start(clock: Clock) -> Running {
        let (gate, opened) = watch::channel(false);
        let gates = Gates { gate: opened, flag: Arc::new(AtomicBool::new(false)), entered: Arc::new(Notify::new()) };
        let (flag, entered) = (Arc::clone(&gates.flag), Arc::clone(&gates.entered));
        let slot = Arc::new(Mutex::new(None));
        let builder = App::builder(Root(gates)).runtime(ulo_runtime_tokio::Tokio::current()).drain_timeout(Duration::from_secs(30));
        let builder = match clock {
            Clock::Frozen => builder.timer(Frozen),
            Clock::SpentCap => builder.shutdown_timeout(Duration::ZERO),
        };
        let app = builder
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(TableServer { table: Arc::clone(&slot) })
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
        let table = slot.lock().unwrap_or_else(PoisonError::into_inner).clone().expect("the server built its table");
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { table, handle, serving, gate, flag, entered }
    }

    /// Connects to `path` over a pipe and sends one message, read by a task that records every
    /// message the client receives until its connection ends, and how it ended.
    async fn message(&self, path: &str) -> JoinHandle<(Vec<Message>, Option<String>)> {
        let Handshake::Switch(switch) = self.table.handshake(head(path), None).await else {
            panic!("the upgrade request to {path} was refused");
        };
        let (server, client) = tokio::io::duplex(64 * 1024);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        switch.serve(async move { Ok::<_, BoxError>(Upgraded::from_tokio(server)) });
        client.send(Message::text("hold")).await.expect("the message was sent");
        tokio::spawn(async move {
            let mut read = Vec::new();
            loop {
                match client.next().await {
                    Some(Ok(message)) => read.push(message),
                    Some(Err(error)) => return (read, Some(error.to_string())),
                    None => return (read, None),
                }
            }
        })
    }

    fn close(&self) -> JoinHandle<Result<Shutdown, ShutdownError>> {
        let handle = self.handle.clone();
        tokio::spawn(async move { handle.close(Signal::new("raw inbox")).await })
    }
}

/// A valid upgrade request's head for `path`.
fn head(path: &str) -> Parts {
    http::Request::builder()
        .method(Method::GET)
        .uri(path)
        .version(Version::HTTP_11)
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .expect("a request head")
        .into_parts()
        .0
}

/// Returns once every task on the test's runtime is waiting: on a paused clock tokio moves time
/// on only then, and nothing else here sleeps on it.
async fn idle() {
    tokio::time::sleep(Duration::from_secs(3600)).await;
}

/// The messages a client read: the gateway's answer, then the drain's close.
fn answered_then_closed(read: &[Message]) -> bool {
    match read {
        [Message::Text(text), Message::Close(Some(frame))] => {
            text.as_str() == "answered" && u16::from(frame.code) == 1001 && frame.reason.as_str() == SHUTDOWN_PHRASE
        }
        _ => false,
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_hand_written_gateways_message_held_on_its_gate_keeps_the_drain_open_until_it_returns() {
    let running = Running::start(Clock::Frozen).await;
    let reading = running.message("/gated").await;
    running.entered.notified().await;

    let closing = running.close();
    idle().await;
    assert!(!closing.is_finished(), "close returned while `on_message` was held on its gate");
    assert!(!reading.is_finished(), "the connection ended while its message was in flight");

    running.gate.send_replace(true);
    let closed = tokio::time::timeout(PATIENCE, closing).await.expect("close returned once the gate opened").expect("the close task");
    let shutdown = closed.unwrap_or_else(|error| panic!("the close failed: {error}"));
    assert_eq!(shutdown.abandoned, 0, "a hand-written gateway's message opens no execution");
    let (read, ended) = tokio::time::timeout(PATIENCE, reading).await.expect("the client's connection ended").expect("the reading task");
    assert!(answered_then_closed(&read), "the client read: {read:?}, then {ended:?}");
    assert_eq!(ended, None, "the connection ended after the close handshake");
    tokio::time::timeout(PATIENCE, running.serving).await.expect("serve returned").expect("the serving task");
}

#[tokio::test(flavor = "current_thread")]
async fn under_a_spent_cap_a_hand_written_gateways_message_ready_unwoken_returns_in_closes_first_poll() {
    let running = Running::start(Clock::SpentCap).await;
    let reading = running.message("/unwoken").await;
    tokio::time::timeout(PATIENCE, running.entered.notified()).await.expect("`on_message` began");
    running.flag.store(true, Ordering::SeqCst);

    let closed = tokio::time::timeout(PATIENCE, running.close()).await.expect("the capped close").expect("the close task");
    let error = closed.expect_err("a close waiting out its grace for the client's close reply is cut by the cap");
    let [failure] = &*error.failures else { panic!("expected one failure, the WebSocket server's close, got: {error}") };
    match failure {
        ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } } => {
            assert_eq!(*transport, TypeName::of::<Ws>(), "the close the cap cut");
            assert_eq!((*after, *limit), (Duration::ZERO, Limit::ShutdownCap), "the WebSocket server's close");
        }
        other => panic!("expected the WebSocket server's close cut by the cap, got: {other:?}"),
    }
    assert_eq!(error.report.abandoned, 0, "a hand-written gateway's message opens no execution");
    let (read, ended) = tokio::time::timeout(PATIENCE, reading).await.expect("the client's connection ended").expect("the reading task");
    assert!(answered_then_closed(&read), "the client read: {read:?}, then {ended:?}");
    tokio::time::timeout(PATIENCE, running.serving).await.expect("serve returned").expect("the serving task");
}
