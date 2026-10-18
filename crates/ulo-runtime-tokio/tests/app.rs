//! Which of `Dep<dyn Timer>` and `Dep<dyn Runtime>` an app binds for each way its builder sets a
//! clock, the refusal of a test's override of either, `listen()`'s refusal of an app that binds a
//! transport with no runtime, and when an app lets go of a server holding its handle: at the
//! close step (F425), and at the app's drop when it never reaches that step (F426). The binding
//! with `.runtime(..)` alone is the conformance suite's `the_app_binds_the_runtime_as_one_object`.

use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ulo::app::Bound as Serving;
use ulo::testing::TestApp;
use ulo::{
    App, AppBuilder, AppHandle, BoundAddr, BoxError, BoxFuture, ConnectError, Connected, DrainToken, FailureReason, LookupError, Module,
    ModuleDef, ModuleIdentity, Mounted, Runtime, RuntimeMissing, Secret, Server, Signal, StartupError, TaskEnd, Timer,
    Transport, WiringError,
};
use ulo_runtime_tokio::Tokio;

/// How long a step that should be immediate may take before the test fails.
const PATIENCE: Duration = Duration::from_secs(5);

/// A test's clock: the time stands at one instant a day ahead, and every sleep ends at once.
struct Frozen(Instant);

impl Frozen {
    fn new() -> Frozen {
        Frozen(Instant::now() + Duration::from_secs(86_400))
    }
}

impl Timer for Frozen {
    fn sleep(&self, _d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }

    fn now(&self) -> Instant {
        self.0
    }
}

struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

async fn connect(builder: AppBuilder) -> App<Connected> {
    builder.wire().expect("an empty app did not wire").connect().await.expect("an empty app did not connect")
}

fn address<T: ?Sized>(value: &T) -> *const () {
    value as *const T as *const ()
}

#[tokio::test]
async fn a_timer_alone_binds_no_runtime() {
    let app = connect(App::builder(Empty).timer(ulo_runtime_tokio::Timer)).await;
    assert!(app.get::<dyn Timer>().await.is_ok(), "`Dep<dyn Timer>` on an app given a timer");
    let runtime = app.get::<dyn Runtime>().await.map(|_| ());
    assert!(
        matches!(runtime, Err(LookupError::NotFound { .. })),
        "`Dep<dyn Runtime>` on an app given a timer alone: {runtime:?}"
    );
    let handle = app.handle();
    assert!(handle.timer().is_some(), "`AppHandle::timer` on an app given a timer");
    assert!(handle.runtime().is_none(), "`AppHandle::runtime` on an app given a timer alone");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn an_app_without_a_clock_hands_out_neither() {
    let app = connect(App::builder(Empty)).await;
    let handle = app.handle();
    assert!(handle.timer().is_none(), "`AppHandle::timer` on an app given no clock");
    assert!(handle.runtime().is_none(), "`AppHandle::runtime` on an app given no clock");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

/// `.runtime(r).timer(t)`: the app spawns on `r` and has `t` as its one clock. `Dep<dyn Timer>`
/// and `AppHandle::timer` are `t`; `Dep<dyn Runtime>` and `AppHandle::runtime` are one object
/// whose clock reads and sleeps on `t` and whose tasks run on `r`.
async fn assert_spawns_on_the_runtime_and_times_with_the_timer(app: App<Connected>, at: Instant) {
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` after `.timer(..)` replaced its clock");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` after `.timer(..)`");
    let handle = app.handle();
    assert_eq!(timer.now(), at, "`Dep<dyn Timer>` is the timer set after the runtime");
    assert_eq!(runtime.now(), at, "`Dep<dyn Runtime>`'s clock is the timer set after the runtime");
    assert_eq!(handle.timer().map(|timer| timer.now()), Some(at), "`AppHandle::timer`");
    assert_eq!(handle.runtime().map(|runtime| runtime.now()), Some(at), "`AppHandle::runtime`'s clock");
    assert_eq!(handle.timer().map(|timer| address(&**timer)), Some(address(&*timer)), "`AppHandle::timer` is `Dep<dyn Timer>`");
    assert_eq!(
        handle.runtime().map(|runtime| address(&**runtime)),
        Some(address(&*runtime)),
        "`AppHandle::runtime` is `Dep<dyn Runtime>`"
    );
    let slept = tokio::time::timeout(PATIENCE, runtime.sleep(Duration::from_secs(3600))).await;
    assert!(slept.is_ok(), "`Dep<dyn Runtime>` slept an hour on the runtime's own clock, not the timer's");
    let task = tokio::time::timeout(PATIENCE, runtime.spawn(Box::pin(async {}))).await;
    assert!(task.as_ref().is_ok_and(TaskEnd::is_finished), "a task spawned through `Dep<dyn Runtime>` ended {task:?}");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

/// `.timer(t).runtime(r)`: the app's wrapper around `r` for both, one object.
async fn assert_one_object_on_the_runtime(app: App<Connected>, at: Instant) {
    let runtime = app.get::<dyn Runtime>().await.expect("`Dep<dyn Runtime>` after `.runtime(..)`");
    let timer = app.get::<dyn Timer>().await.expect("`Dep<dyn Timer>` after `.runtime(..)`");
    assert_eq!(address(&*timer), address(&*runtime), "`Dep<dyn Timer>` is the runtime set after the timer");
    assert_ne!(timer.now(), at, "`Dep<dyn Timer>` still reads the timer `.runtime(..)` replaced");
    let handle = app.handle();
    assert_eq!(handle.timer().map(|timer| address(&**timer)), Some(address(&*runtime)), "`AppHandle::timer`");
    assert_eq!(handle.runtime().map(|runtime| address(&**runtime)), Some(address(&*runtime)), "`AppHandle::runtime`");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

/// A lazily loaded module registering a secret, and nothing else.
struct Lazy(Secret<String>);

impl Module for Lazy {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.secret(&self.0);
    }
}

/// The app's runtime redacts with the graph a `load` publishes, so a secret registered after
/// startup is replaced in the panic of a task spawned through it.
#[tokio::test]
async fn a_secret_a_loaded_module_registers_is_redacted_from_a_task_s_panic() {
    let app = connect(App::builder(Empty).runtime(Tokio::current())).await;
    let handle = app.handle();
    handle.load(Lazy(Secret::new("loaded-token-91c2".to_owned()))).await.expect("a module registering a secret did not load");
    let runtime = Arc::clone(handle.runtime().expect("`AppHandle::runtime` on an app given a runtime"));
    let task = runtime.spawn(Box::pin(async { panic!("a deliberate panic carrying loaded-token-91c2") }));
    let end = tokio::time::timeout(PATIENCE, task).await.expect("a task that panics did not end");
    let TaskEnd::Panicked(message) = &end else {
        panic!("a task that panicked ended {end:?}");
    };
    assert_eq!(message.to_string(), "a deliberate panic carrying [redacted]", "the secret the loaded module registered");
    app.close(Signal::new("test")).await.expect("the app did not close");
}

#[tokio::test]
async fn a_timer_after_a_runtime_replaces_only_its_clock() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = connect(App::builder(Empty).runtime(Tokio::current()).timer(frozen)).await;
    assert_spawns_on_the_runtime_and_times_with_the_timer(app, at).await;
}

#[tokio::test]
async fn a_runtime_after_a_timer_replaces_both() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = connect(App::builder(Empty).timer(frozen).runtime(Tokio::current())).await;
    assert_one_object_on_the_runtime(app, at).await;
}

#[tokio::test]
async fn a_test_s_timer_after_its_runtime_replaces_only_its_clock() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = TestApp::of(Empty).runtime(Tokio::current()).timer(frozen).connect().await.expect("the test app connects");
    assert_spawns_on_the_runtime_and_times_with_the_timer(app, at).await;
}

#[tokio::test]
async fn a_test_s_runtime_after_its_timer_replaces_both() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let app = TestApp::of(Empty).timer(frozen).runtime(Tokio::current()).connect().await.expect("the test app connects");
    assert_one_object_on_the_runtime(app, at).await;
}

/// A module whose init hook never returns, under `hook_timeout`.
struct Hangs;

impl Module for Hangs {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.on_init(std::future::pending::<Result<(), BoxError>>);
    }
}

/// The app's own waits read the timer set after the runtime: an hour's `hook_timeout` expires at
/// once on a clock whose sleeps end at once, where the runtime's own clock would wait the hour.
#[tokio::test]
async fn the_app_s_waits_read_the_timer_set_after_the_runtime() {
    let builder = App::builder(Hangs).runtime(Tokio::current()).timer(Frozen::new()).hook_timeout(Duration::from_secs(3600));
    let wired = builder.wire().expect("the app wires");
    let connected = tokio::time::timeout(PATIENCE, wired.connect())
        .await
        .expect("the init hook ran on the runtime's own clock, not the timer's");
    let Err(StartupError::Connect(ConnectError::Hook { reason, .. })) = connected else {
        panic!("a hook that never returns did not fail the connect phase");
    };
    assert!(matches!(reason, FailureReason::TimedOut { .. }), "the hook failed otherwise: {reason}");
}

#[tokio::test]
async fn overriding_the_runtime_is_refused_naming_the_test_builder() {
    let wired = TestApp::of(Empty).runtime(Tokio::current()).override_value::<dyn Runtime>(Arc::new(Tokio::current())).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an override of `dyn Runtime` wired");
    };
    assert!(
        errors.iter().any(|error| matches!(error, WiringError::RuntimeOverride { .. })),
        "the errors: {errors}"
    );
    let text = errors.to_string();
    assert!(text.contains("help: set it with `TestApp::runtime(..)`"), "the refusal's hint does not name the test builder: {text}");
}

#[tokio::test]
async fn overriding_the_timer_is_refused_naming_the_test_builder() {
    let wired = TestApp::of(Empty).timer(ulo_runtime_tokio::Timer).override_value::<dyn Timer>(Arc::new(ulo_runtime_tokio::Timer)).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("an override of `dyn Timer` wired");
    };
    assert!(errors.iter().any(|error| matches!(error, WiringError::TimerOverride { .. })), "the errors: {errors}");
    let text = errors.to_string();
    assert!(text.contains("help: set it with `TestApp::timer(..)`"), "the refusal's hint does not name the test builder: {text}");
}

/// A transport with no handlers, for a server that binds nothing.
struct Bare;

impl Transport for Bare {
    const KEY: &'static str = "bare";
    type Cx = ();
    type Reply = ();
}

/// What a [`BareServer`] saw: whether `prepare` ran, the address of the runtime it was handed, and
/// the time its timer and its runtime read.
#[derive(Default)]
struct Prepared {
    ran: AtomicBool,
    runtime: Mutex<Option<usize>>,
    clocks: Mutex<Option<(Instant, Instant)>>,
}

/// A server that binds nothing and records its `prepare`.
struct BareServer(Arc<Prepared>);

impl Server for BareServer {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.0.ran.store(true, Ordering::SeqCst);
        *self.0.runtime.lock().unwrap_or_else(PoisonError::into_inner) = Some(address(&**mounted.runtime()) as usize);
        *self.0.clocks.lock().unwrap_or_else(PoisonError::into_inner) = Some((mounted.timer().now(), mounted.runtime().now()));
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Ok(())
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// `listen()`'s answer for `builder` binding a [`BareServer`], and whether its `prepare` ran.
async fn listen_bare(builder: AppBuilder) -> (Result<App<Serving>, StartupError>, Arc<Prepared>) {
    let prepared = Arc::new(Prepared::default());
    let listened = connect(builder).await.bind(BareServer(Arc::clone(&prepared))).listen().await;
    (listened, prepared)
}

fn assert_runtime_missing(listened: Result<App<Serving>, StartupError>, prepared: &Prepared, what: &str) {
    let Err(StartupError::Bind { transport, source }) = listened else {
        panic!("{what} binding a transport was not refused at `listen()`");
    };
    assert_eq!(transport.to_string(), "Bare", "the refusal names the transport bound");
    assert!(source.downcast_ref::<RuntimeMissing>().is_some(), "the refusal is not a `RuntimeMissing`: {source}");
    let text = source.to_string();
    assert!(text.contains("no `Runtime`") && text.contains("`.runtime(..)`"), "the refusal does not name `.runtime(..)`: {text}");
    assert!(!prepared.ran.load(Ordering::SeqCst), "a server on {what} was prepared before the refusal");
}

#[tokio::test]
async fn an_app_with_a_timer_alone_is_refused_when_it_binds_a_transport() {
    let (listened, prepared) = listen_bare(App::builder(Empty).timer(ulo_runtime_tokio::Timer)).await;
    assert_runtime_missing(listened, &prepared, "an app with a timer alone");
}

#[tokio::test]
async fn an_app_without_a_clock_is_refused_when_it_binds_a_transport() {
    let (listened, prepared) = listen_bare(App::builder(Empty)).await;
    assert_runtime_missing(listened, &prepared, "an app without a clock");
}

#[tokio::test]
async fn a_runtime_is_the_one_a_server_is_handed() {
    let (listened, prepared) = listen_bare(App::builder(Empty).runtime(Tokio::current())).await;
    let app = listened.unwrap_or_else(|error| panic!("an app with a runtime binding a transport was refused: {error}"));
    let handed = *prepared.runtime.lock().unwrap_or_else(PoisonError::into_inner);
    let runtime = app.handle().runtime().map(|runtime| address(&**runtime) as usize);
    assert!(handed.is_some(), "the server was not prepared");
    assert_eq!(handed, runtime, "`Mounted::runtime` is not the app's runtime");
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
}

#[tokio::test]
async fn a_timer_alone_is_enough_for_an_app_that_binds_nothing() {
    let app = connect(App::builder(Empty).timer(ulo_runtime_tokio::Timer)).await;
    let app = app.listen().await.unwrap_or_else(|error| panic!("an app binding no transport was refused: {error}"));
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
}

#[tokio::test]
async fn a_server_times_with_the_timer_set_after_the_runtime() {
    let frozen = Frozen::new();
    let at = frozen.0;
    let (listened, prepared) = listen_bare(App::builder(Empty).runtime(Tokio::current()).timer(frozen)).await;
    let app = listened.unwrap_or_else(|error| panic!("an app with a runtime binding a transport was refused: {error}"));
    let clocks = *prepared.clocks.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(clocks, Some((at, at)), "`Mounted::timer` and `Mounted::runtime` read the timer set after the runtime");
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
}

/// Sets its flag when it drops.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A server keeping the `AppHandle` its `prepare` is handed, as the shipped transports' servers
/// do to open executions, so it holds the app.
struct Holding {
    app: Option<AppHandle>,
    _dropped: DropFlag,
}

impl Server for Holding {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.app = Some(mounted.app().clone());
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Ok(())
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        vec![BoundAddr::new("bare", "127.0.0.1:7000".parse().expect("an address"))]
    }
}

/// A module whose one singleton sets its flag when it drops.
struct Flagged(Arc<AtomicBool>);

impl Module for Flagged {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(DropFlag(Arc::clone(&self.0)));
    }
}

/// F425: the app holds its servers and a server holding the app's handle holds the app, so
/// neither dropped. A closed app lets go of its servers: the server drops once `serve` has
/// returned, a handle still held, and the app's singletons once the last handle drops.
#[tokio::test]
async fn a_closed_app_drops_a_server_holding_its_handle() {
    let (server, singleton) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let app = connect(App::builder(Flagged(Arc::clone(&singleton))).runtime(Tokio::current()))
        .await
        .bind(Holding { app: None, _dropped: DropFlag(Arc::clone(&server)) })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    let handle = app.handle();
    app.serve(async { Signal::new("test") }).await.expect("the app did not shut down");
    assert!(server.load(Ordering::SeqCst), "the server outlived the closed app's `serve`");
    assert!(!singleton.load(Ordering::SeqCst), "the app's singletons dropped while a handle was held");
    drop(handle);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived its last handle");
}

/// `App<Bound>::addresses()` lists the server's address until the close step has run, then
/// nothing, though the app is still held.
#[tokio::test]
async fn a_closed_app_lists_no_addresses() {
    let app = connect(App::builder(Flagged(Arc::new(AtomicBool::new(false)))).runtime(Tokio::current()))
        .await
        .bind(Holding { app: None, _dropped: DropFlag(Arc::new(AtomicBool::new(false))) })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    assert_eq!(app.addresses().len(), 1, "the bound server's address is missing before the close");
    let handle = app.handle();
    handle.close(Signal::new("test")).await.expect("the app did not shut down");
    assert!(app.addresses().is_empty(), "the closed app still lists {:?}", app.addresses());
}

/// A `close` on a handle of an app that was bound and never served drops its server inside the
/// close sequence, and a later `serve` returns at once.
#[tokio::test]
async fn a_close_on_an_unserved_app_drops_its_server() {
    let (server, singleton) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let app = connect(App::builder(Flagged(Arc::clone(&singleton))).runtime(Tokio::current()))
        .await
        .bind(Holding { app: None, _dropped: DropFlag(Arc::clone(&server)) })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    let handle = app.handle();
    assert!(!server.load(Ordering::SeqCst), "the server dropped before the close");
    handle.close(Signal::new("test")).await.expect("the app did not shut down");
    assert!(server.load(Ordering::SeqCst), "the server outlived the close sequence");
    tokio::time::timeout(PATIENCE, app.serve(std::future::pending::<Signal>()))
        .await
        .expect("`serve` on a closed app did not return")
        .expect("`serve` on a closed app failed");
    drop(handle);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived its last handle");
}

/// What a [`Listening`] server saw: its closes, its drops and the address it bound; and, while
/// `panics` is set, a `bound` that panics once.
#[derive(Default)]
struct Seen {
    closes: AtomicUsize,
    drops: AtomicUsize,
    addr: Mutex<Option<SocketAddr>>,
    panics: AtomicBool,
}

impl Seen {
    fn closes(&self) -> usize {
        self.closes.load(Ordering::SeqCst)
    }

    fn drops(&self) -> usize {
        self.drops.load(Ordering::SeqCst)
    }

    fn addr(&self) -> SocketAddr {
        self.addr.lock().unwrap_or_else(PoisonError::into_inner).expect("the server bound no address")
    }
}

/// A server keeping the `AppHandle` its `prepare` is handed and a TCP listener it binds, serving
/// until its future is dropped.
struct Listening {
    app: Option<AppHandle>,
    listener: Option<TcpListener>,
    seen: Arc<Seen>,
}

impl Drop for Listening {
    fn drop(&mut self) {
        self.seen.drops.fetch_add(1, Ordering::SeqCst);
    }
}

impl Server for Listening {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.app = Some(mounted.app().clone());
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        *self.seen.addr.lock().unwrap_or_else(PoisonError::into_inner) = Some(listener.local_addr()?);
        self.listener = Some(listener);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        self.seen.closes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        if self.seen.panics.swap(false, Ordering::SeqCst) {
            panic!("`bound` panics while the app's list of servers is locked");
        }
        vec![BoundAddr::new("bare", self.seen.addr())]
    }
}

/// A module whose one singleton sets its flag when it drops, and whose before-shutdown hook counts
/// its runs and, when `stalls`, never returns.
struct Hooked {
    singleton: Arc<AtomicBool>,
    runs: Arc<AtomicUsize>,
    stalls: bool,
}

impl Module for Hooked {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(DropFlag(Arc::clone(&self.singleton)));
        let (runs, stalls) = (Arc::clone(&self.runs), self.stalls);
        m.before_shutdown(move |_signal: Signal| {
            let runs = Arc::clone(&runs);
            async move {
                runs.fetch_add(1, Ordering::SeqCst);
                if stalls {
                    std::future::pending::<()>().await;
                }
            }
        });
    }
}

/// An app binding one [`Listening`] server, with what the server and the module record.
struct Watched {
    app: App<Serving>,
    seen: Arc<Seen>,
    singleton: Arc<AtomicBool>,
    runs: Arc<AtomicUsize>,
}

async fn listening(stalls: bool) -> Watched {
    let seen = Arc::new(Seen::default());
    let (singleton, runs) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
    let root = Hooked { singleton: Arc::clone(&singleton), runs: Arc::clone(&runs), stalls };
    let app = connect(App::builder(root).runtime(Tokio::current()))
        .await
        .bind(Listening { app: None, listener: None, seen: Arc::clone(&seen) })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    assert!(dial(seen.addr()).is_ok(), "the bound server's address refused a connection");
    Watched { app, seen, singleton, runs }
}

fn dial(addr: SocketAddr) -> std::io::Result<TcpStream> {
    TcpStream::connect_timeout(&addr, Duration::from_secs(1))
}

/// Refused, and not failed otherwise: a listener still open whose backlog is full fails an
/// attempt too, with a reset or a timeout by platform.
fn refuses(addr: SocketAddr) -> bool {
    dial(addr).is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused)
}

/// What F426's three paths share once the app's value is gone: the server has dropped, its
/// listener with it, without its `close` having run; the singletons drop with the last handle.
fn assert_released(seen: &Seen, singleton: &AtomicBool, handle: AppHandle, path: &str) {
    assert_eq!(seen.drops(), 1, "the server outlived {path}");
    assert!(refuses(seen.addr()), "the server's address did not refuse a connection after {path}");
    assert_eq!(seen.closes(), 0, "the server's `close` ran after {path}");
    assert!(!singleton.load(Ordering::SeqCst), "the app's singletons dropped while a handle was held");
    drop(handle);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived its last handle after {path}");
}

/// F426: an app bound and dropped without `serve` or `close` lets go of its server as it drops.
#[tokio::test]
async fn a_bound_app_dropped_unserved_drops_its_server() {
    let Watched { app, seen, singleton, .. } = listening(false).await;
    let handle = app.handle();
    drop(app);
    assert_released(&seen, &singleton, handle, "the app dropped unserved");
}

/// F426: an app whose `serve` future is dropped before any trigger, here by a timeout, lets go of
/// its server as the future drops the app.
#[tokio::test]
async fn an_app_whose_serve_is_dropped_drops_its_server() {
    let Watched { app, seen, singleton, runs } = listening(false).await;
    let handle = app.handle();
    let served = tokio::time::timeout(Duration::from_millis(50), app.serve(std::future::pending::<Signal>())).await;
    assert!(served.is_err(), "`serve` returned with no trigger");
    assert_eq!(runs.load(Ordering::SeqCst), 0, "the shutdown started");
    assert_released(&seen, &singleton, handle, "`serve`'s dropped future");
}

/// F426: a shutdown whose runner, `serve` here, is dropped inside a before-shutdown hook that never
/// returns, and never resumed, lets go of the server as the runner drops the app.
#[tokio::test]
async fn an_app_whose_shutdown_is_abandoned_drops_its_server() {
    let Watched { app, seen, singleton, runs } = listening(true).await;
    let handle = app.handle();
    let served = tokio::time::timeout(Duration::from_millis(200), app.serve(async { Signal::new("test") })).await;
    assert!(served.is_err(), "`serve` returned through a hook that never returns");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "the shutdown did not reach its before-shutdown hook");
    assert_released(&seen, &singleton, handle, "the abandoned shutdown");
}

/// A closed app's drop releases nothing again and does not panic, with the app's list of servers
/// poisoned by a panicking `bound`; a later `close` on a handle answers the same outcome and runs
/// no hook and no `close` again.
#[tokio::test]
async fn a_closed_app_s_drop_and_a_later_close_release_nothing_again() {
    let Watched { app, seen, singleton, runs } = listening(false).await;
    let handle = app.handle();
    seen.panics.store(true, Ordering::SeqCst);
    let listed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.addresses()));
    assert!(listed.is_err(), "`bound` did not panic inside `addresses()`");
    let closed = handle.close(Signal::new("first")).await.expect("the app did not shut down");
    assert_eq!((seen.closes(), seen.drops()), (1, 1), "the close step did not close and release the server once");
    drop(app);
    assert_eq!((seen.closes(), seen.drops()), (1, 1), "the closed app's drop closed or released the server again");
    let again = handle.close(Signal::new("second")).await.expect("a close after the app's drop failed");
    assert_eq!(again.signal.name(), closed.signal.name(), "a later close answered another outcome");
    assert_eq!(runs.load(Ordering::SeqCst), 1, "a later close ran the before-shutdown hook again");
    assert_eq!(seen.closes(), 1, "a later close ran the server's `close` again");
    drop(handle);
    assert!(singleton.load(Ordering::SeqCst), "the app's singletons outlived its last handle");
}
