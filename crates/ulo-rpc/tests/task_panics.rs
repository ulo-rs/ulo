//! A panic in a link's own long-lived task (transports DESIGN §5.3, 168): a server's lane ends
//! its server as a transport failure whose outcome holds the redacted panic, and a client's reply
//! router fails the calls waiting on the connection `Unavailable` holding it, then connects again.
//! The links here spawn their tasks through `TaskPanics` as the broker links do, each task held at
//! a gate the test opens, so the panic comes while the server serves or while a call waits.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use ulo::{App, AppHandle, BoxError, Module, ModuleDef, ModuleIdentity, Runtime, Secret, ServeEnd, ShutdownFailure, Signal};
use ulo_rpc::link::{Inbound, TaskPanics};
use ulo_rpc::{Capabilities, Codec, DeliveryMode, ErrorKind, Frame, Link, Outbound, RpcClient, Subscription};
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);

/// A value the app registers, which no report may print.
const SECRET: &str = "lane-token-5c1e";

/// What the planted panics say: the app's secret and a url's userinfo.
const PANIC_TEXT: &str = "reading frames for nats://ulo:hunter2@broker.internal with lane-token-5c1e";

/// A link whose server side spawns one watched "request lane" and whose client side spawns one
/// watched "reply router", each panicking with [`PANIC_TEXT`] once its gate opens.
struct Panicking {
    app_runtime: Mutex<Option<Arc<dyn Runtime>>>,
    lane: Mutex<Option<oneshot::Receiver<()>>>,
    router: Mutex<Option<oneshot::Receiver<()>>>,
    connects: Arc<AtomicU64>,
}

impl Panicking {
    fn new(lane: Option<oneshot::Receiver<()>>, router: Option<oneshot::Receiver<()>>) -> Self {
        Panicking { app_runtime: Mutex::new(None), lane: Mutex::new(lane), router: Mutex::new(router), connects: Arc::default() }
    }
}

/// A task that holds `frames` open and panics once `gate` opens, or never without one.
async fn panics_at(gate: Option<oneshot::Receiver<()>>, frames: mpsc::UnboundedSender<Frame>) {
    let _frames = frames;
    match gate {
        Some(gate) => {
            let _ = gate.await;
            panic!("{PANIC_TEXT}");
        }
        None => std::future::pending().await,
    }
}

impl Link for Panicking {
    const NAME: &'static str = "panicking";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).codec(Codec::Json).without_every_instance("the test link addresses one server")
    }

    async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
        *lock(&self.app_runtime) = app.runtime().cloned();
        Ok(())
    }

    async fn listen(&self, _subscriptions: &[Subscription]) -> Result<Inbound, BoxError> {
        let runtime = lock(&self.app_runtime).clone().ok_or("listen before prepare")?;
        let panics = TaskPanics::new("Panicking", runtime);
        let (deliveries, mut received) = mpsc::unbounded_channel::<ulo_rpc::Delivery>();
        let (frames, _) = mpsc::unbounded_channel();
        // Holding its own sender, the stream never ends by itself.
        let inbound = panics.guard(futures_util::stream::poll_fn(move |cx| {
            let _ = &deliveries;
            received.poll_recv(cx)
        }));
        tokio::spawn(panics.watch_feeder("request lane", panics_at(lock(&self.lane).take(), frames)));
        Ok(inbound)
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let panics = TaskPanics::new("Panicking", Arc::clone(runtime));
        let (frames, replies) = mpsc::unbounded_channel::<Frame>();
        let mut replies = replies;
        let replies = panics.guard(futures_util::stream::poll_fn(move |cx| replies.poll_recv(cx)));
        tokio::spawn(panics.watch_feeder("reply router", panics_at(lock(&self.router).take(), frames)));
        Ok(Outbound { send: Box::new(|_pattern, _frame, _reply_to| Box::pin(async { Ok(()) })), replies })
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.secret(&Secret::new(SECRET.to_owned()));
    }
}

/// Neither the url's password nor the app's secret, and the panic's text otherwise.
fn redacted(text: &str) {
    assert!(!text.contains("hunter2"), "the url's password reached the report: {text}");
    assert!(text.contains("the Panicking link's"), "the report does not name the link: {text}");
    assert!(text.contains("reading frames for nats://[redacted]@broker.internal"), "the panic's own text is missing: {text}");
}

/// A server whose link's lane panics while it serves ends as a transport failure: `serve` answers
/// `Err` whose failure is `Serve` holding `ServeEnd::Panicked`, the message naming the link and the
/// task with the app's secret replaced and the url's password stripped, and `ulo::is_panic` true.
#[tokio::test]
async fn a_panicking_lane_ends_its_server_as_a_panicked_failure_the_core_recognises() {
    let (open, gate) = oneshot::channel();
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Panicking::new(Some(gate), None)))
        .listen()
        .await
        .map_err(Box::new)
        .expect("the server binds");
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!serving.is_finished(), "the server stopped before its lane panicked");
    open.send(()).expect("the lane waits at its gate");
    let outcome = tokio::time::timeout(PATIENCE, serving).await.expect("`serve` ended in time").expect("the serving task completes");
    let error = outcome.expect_err("a server whose lane panicked shut down cleanly");
    let [ShutdownFailure::Serve { end: ServeEnd::Panicked(message), .. }] = &*error.failures else {
        panic!("the shutdown's failure is not a panicked serve: {error}");
    };
    let text = message.to_string();
    redacted(&text);
    assert!(text.starts_with("the Panicking link's request lane panicked: "), "{text}");
    assert!(!text.contains(SECRET), "the app's secret reached the report: {text}");
    assert!(ulo::is_panic(&BoxError::from(error)), "the shutdown's outcome does not hold the panic");
}

/// A client whose link's reply router panics while a call waits fails that call `Unavailable`
/// holding the panic, the url's password stripped, `ulo::is_panic` true of the call's error; the
/// next call connects again.
#[tokio::test]
async fn a_panicking_reply_router_fails_the_waiting_call_holding_the_redacted_panic() {
    let (open, gate) = oneshot::channel();
    let link = Panicking::new(None, Some(gate));
    let connects = Arc::clone(&link.connects);
    let client = RpcClient::new(link, Arc::new(Tokio::current())).expect("the link is usable");
    let caller = client.clone();
    let calling = tokio::spawn(async move { caller.request::<_, u32>("lanes.call", &()).await });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!calling.is_finished(), "the call ended before its reply router panicked");
    open.send(()).expect("the router waits at its gate");
    let failed = tokio::time::timeout(PATIENCE, calling).await.expect("the call ended in time").expect("the call's task completes");
    let failed = failed.expect_err("a call over a panicked reply router succeeded");
    assert_eq!(failed.kind(), ErrorKind::Unavailable, "{failed}");
    let text = failed.to_string();
    redacted(&text);
    assert!(text.contains("the Panicking link's reply router panicked: "), "{text}");
    assert!(ulo::is_panic(&BoxError::from(failed)), "the call's error does not hold the panic: {text}");

    let next = tokio::time::timeout(Duration::from_millis(200), client.request::<_, u32>("lanes.call", &()).timeout(Duration::from_millis(100))).await;
    assert!(next.is_ok(), "the next call did not end within its own timeout");
    assert_eq!(connects.load(Ordering::SeqCst), 2, "the next call did not connect again");
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
