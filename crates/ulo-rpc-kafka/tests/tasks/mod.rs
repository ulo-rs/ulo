//! F585: a panic in one of the link's own tasks, planted through `panic_in`, ends a server as a
//! transport failure and fails a waiting client call, each holding the panic redacted; and the
//! link's tasks end with their owner, closed or dropped, counted on a runtime of the link's own.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use ulo::{App, BoxError, Dep, Module, ModuleDef, ModuleIdentity, Secret, ServeEnd, ShutdownFailure, Signal, controller, handlers};
use ulo_rpc::link::PlantedPanic;
use ulo_rpc::{ErrorKind, Link, RpcClient};
use ulo_runtime_tokio::Tokio;
use ulo_transport::Classify;

const PATIENCE: Duration = Duration::from_secs(30);

/// A value the app registers, which no report may print.
const SECRET: &str = "task-token-9d2f";

/// What a planted panic says: the app's secret and a url's userinfo.
pub const PANIC_TEXT: &str = "the lane met amqp://ulo:hunter2@broker.internal holding task-token-9d2f";

pub const HOLD: &str = "tasks.hold";
pub const ECHO: &str = "tasks.echo";

#[derive(Debug)]
pub struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("never raised")
    }
}

impl std::error::Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// Raised once `tasks.hold`'s handler runs.
#[derive(Clone, Default)]
pub struct Entered(Arc<Notify>);

#[controller]
pub struct Held {
    entered: Dep<Entered>,
}

#[handlers]
impl Held {
    /// Answers nothing, so its call waits while the test plants a panic.
    #[ulo_rpc::message("tasks.hold")]
    async fn hold(&self) -> Result<u32, Never> {
        self.entered.0.notify_one();
        std::future::pending().await
    }

    #[ulo_rpc::message("tasks.echo")]
    async fn echo(&self) -> Result<u32, Never> {
        Ok(7)
    }
}

pub struct Root(pub Entered);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Held>();
        m.secret(&Secret::new(SECRET.to_owned()));
    }
}

async fn bound<L: Link>(link: L, entered: Entered) -> App<ulo::app::Bound> {
    App::builder(Root(entered))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(link))
        .listen()
        .await
        .map_err(Box::new)
        .expect("the server binds")
}

/// Neither the url's password nor `secret`, when given, and the text naming the link and the
/// task.
fn redacted(text: &str, expect: &str, secret: bool) {
    assert!(text.contains(expect), "the report does not name the link and the task, `{expect}`: {text}");
    assert!(!text.contains("hunter2"), "the url's password reached the report: {text}");
    assert!(text.contains("amqp://[redacted]@broker.internal"), "the panic's own text is missing: {text}");
    if secret {
        assert!(!text.contains(SECRET), "the app's secret reached the report: {text}");
    }
}

/// A server whose link's `plant` fires while it serves ends: `serve` answers `Err` whose
/// failure is `ServeEnd::Panicked`, the message beginning with `expect`, the app's secret and
/// the url's password replaced, and `ulo::is_panic` true of it.
pub async fn server_task_panics<L: Link>(link: L, plant: PlantedPanic, expect: &str) {
    let app = bound(link, Entered::default()).await;
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    plant.fire();
    let outcome = tokio::time::timeout(PATIENCE, serving).await.expect("`serve` ended in time").expect("the serving task completes");
    let error = outcome.expect_err("a server whose task panicked shut down cleanly");
    let [ShutdownFailure::Serve { end: ServeEnd::Panicked(message), .. }] = &*error.failures else {
        panic!("the shutdown's failure is not a panicked serve: {error}");
    };
    let text = message.to_string();
    redacted(&text, expect, true);
    assert!(text.starts_with(expect), "{text}");
    assert!(ulo::is_panic(&BoxError::from(error)), "the shutdown's outcome does not hold the panic");
}

/// A client whose link's `plant` fires while a call waits on its server fails that call
/// `Unavailable` holding the panic, the url's password stripped, `ulo::is_panic` true of the
/// call's error. The plant stays fired, so the task of that name the next call's connection
/// spawns panics too; `ulo-rpc`'s `tests/task_panics.rs` holds the reconnect.
pub async fn client_task_panics<L: Link>(server: L, client: L, plant: PlantedPanic, expect: &str) {
    let entered = Entered::default();
    let app = bound(server, entered.clone()).await;
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    let rpc = RpcClient::new(client, Arc::new(Tokio::current())).expect("the link is usable");
    let caller = rpc.clone();
    let calling = tokio::spawn(async move { caller.request::<_, u32>(HOLD, &()).timeout(PATIENCE).await });
    tokio::time::timeout(PATIENCE, entered.0.notified()).await.expect("the call reached its handler");
    plant.fire();
    let failed = tokio::time::timeout(PATIENCE, calling).await.expect("the call ended in time").expect("the call's task completes");
    let failed = failed.expect_err("a call over a panicked task succeeded");
    assert_eq!(failed.kind(), ErrorKind::Unavailable, "{failed}");
    let text = failed.to_string();
    redacted(&text, expect, false);
    assert!(ulo::is_panic(&BoxError::from(failed)), "the call's error does not hold the panic: {text}");
    drop(rpc);
    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("the app closed in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
}

/// Which tasks a cycle requires to end: the link's own, counted through its `live_tasks`, and,
/// on a link whose client library leaves none of its own behind a closed connection, every task
/// on the links' runtime.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code, reason = "each link's suite names one of the two")]
pub enum Counted {
    LinkAndRuntime,
    /// lapin 4.12 leaves two tasks of its own on the runtime for each connection it opened and
    /// closed, so the runtime's count is not the link's.
    LinkOnly,
}

/// Three cycles on links whose tasks run on a runtime of their own: an app serving and closed,
/// with a client answered and dropped; an app bound and dropped unserved; and a client's link
/// connected through `Link::connect` and dropped without `close`. Each requires a link task alive
/// while it holds, and none within [`PATIENCE`] once it is over: the links' own, counted by
/// `live`, and, under [`Counted::LinkAndRuntime`], none on the links' runtime either.
pub async fn cycles_leave_no_task_running<L: Link>(
    link: impl Fn(tokio::runtime::Handle) -> L,
    live: impl Fn(&L) -> Arc<AtomicUsize>,
    counted: Counted,
) {
    let tasks = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("the links' runtime");
    let counters: Mutex<Vec<Arc<AtomicUsize>>> = Mutex::default();
    let make = || {
        let made = link(tasks.handle().clone());
        counters.lock().unwrap_or_else(PoisonError::into_inner).push(live(&made));
        made
    };
    let alive = || {
        let links: usize = counters.lock().unwrap_or_else(PoisonError::into_inner).iter().map(|live| live.load(Ordering::Acquire)).sum();
        (links, tasks.metrics().num_alive_tasks())
    };
    let ended = || {
        let (links, runtime) = alive();
        links == 0 && (counted == Counted::LinkOnly || runtime == 0)
    };

    let app = bound(make(), Entered::default()).await;
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    let rpc = RpcClient::new(make(), Arc::new(Tokio::current())).expect("the link is usable");
    let answered = tokio::time::timeout(PATIENCE, rpc.request::<_, u32>(ECHO, &()).timeout(PATIENCE)).await.expect("answered in time");
    assert_eq!(answered.expect("the call is answered"), 7);
    assert!(alive().0 > 0, "no link task ran while the links served: {:?}", alive());
    drop(rpc);
    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("the app closed in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
    none_alive("an app closed and a client dropped", &ended, &alive).await;

    let app = bound(make(), Entered::default()).await;
    assert!(alive().0 > 0, "no link task ran while the server was bound: {:?}", alive());
    drop(app);
    none_alive("an app bound and dropped unserved", &ended, &alive).await;

    let client = make();
    let runtime: Arc<dyn ulo::Runtime> = Arc::new(Tokio::current());
    let outbound = tokio::time::timeout(PATIENCE, client.connect(&runtime)).await.expect("connected in time").expect("the client's link connects");
    assert!(alive().0 > 0, "no link task ran while the client was connected: {:?}", alive());
    drop(outbound);
    drop(client);
    none_alive("a client's link dropped without `close`", &ended, &alive).await;

    tasks.shutdown_background();
}

async fn none_alive(after: &str, ended: &impl Fn() -> bool, alive: &impl Fn() -> (usize, usize)) {
    let waited = tokio::time::timeout(PATIENCE, async {
        while !ended() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let (links, runtime) = alive();
    assert!(waited.is_ok(), "{links} link tasks, and {runtime} tasks on the links' runtime, still alive {PATIENCE:?} after {after}");
}
