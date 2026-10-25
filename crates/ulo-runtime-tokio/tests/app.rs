//! Which of `Dep<dyn Timer>` and `Dep<dyn Runtime>` an app binds for each way its builder sets a
//! clock, the refusal of a test's override of either, `listen()`'s refusal of an app that binds a
//! transport with no runtime, and when an app lets go of a server holding its handle: at the
//! close step (F425), at the app's drop when it never reaches that step (F426), and, when the app
//! drops while a handle's `close` holds its claim, taken at the call, at that close's close step,
//! or as it drops if it is abandoned or dropped unpolled first; a `close` called after such a drop
//! answers `ShutdownFailure::Dropped`. Such a drop moves the phase to `Closed` and resolves
//! `draining()` for a loop holding a handle. A transport whose `serve` ends while the app runs
//! starts the shutdown, and `serve` answers `ShutdownFailure::Serve` naming it: the first bound
//! when two end on one poll, and none when the end follows a `close`. The binding with
//! `.runtime(..)` alone is the conformance suite's `the_app_binds_the_runtime_as_one_object`.

use std::future::{Future, poll_fn};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use ulo::app::Bound as Serving;
use ulo::testing::TestApp;
use ulo::{
    App, AppBuilder, AppHandle, BoundAddr, BoxError, BoxFuture, ConnectError, Connected, DrainToken, FailureReason, LookupError, Module,
    ModuleDef, ModuleIdentity, Mounted, Phase, Runtime, RuntimeMissing, Secret, ServeEnd, Server, ShutdownFailure, Signal, StartupError,
    TaskEnd, Timer, Transport, TypeName, WiringError,
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

/// What happened, in order, across a [`Recording`] server pair and a [`Gated`] module.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    fn push(&self, event: impl Into<String>) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event.into());
    }

    fn events(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn has(&self, event: &str) -> bool {
        self.events().iter().any(|seen| seen == event)
    }
}

/// A gate a future waits at until the test opens it.
struct Gate(Mutex<(bool, Vec<Waker>)>);

impl Gate {
    fn new(open: bool) -> Arc<Gate> {
        Arc::new(Gate(Mutex::new((open, Vec::new()))))
    }

    fn open(&self) {
        let waiting = {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        waiting.into_iter().for_each(Waker::wake);
    }

    async fn pass(&self) {
        poll_fn(|cx| {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if state.0 {
                return Poll::Ready(());
            }
            state.1.push(cx.waker().clone());
            Poll::Pending
        })
        .await
    }
}

/// A server keeping the `AppHandle` its `prepare` is handed, as every shipped server does, whose
/// `drain` and `close` record that they ran and wait at their gates, and whose drop is logged.
struct Recording {
    name: &'static str,
    app: Option<AppHandle>,
    log: Log,
    drain_gate: Arc<Gate>,
    close_gate: Arc<Gate>,
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.log.push(format!("drop {}", self.name));
    }
}

impl Server for Recording {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.app = Some(mounted.app().clone());
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: DrainToken) {
        self.log.push(format!("drain {}", self.name));
        self.drain_gate.pass().await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.log.push(format!("close {}", self.name));
        self.close_gate.pass().await;
        Ok(())
    }
}

/// A value whose drop is logged.
struct Logged(Log, &'static str);

impl Drop for Logged {
    fn drop(&mut self) {
        self.0.push(self.1);
    }
}

/// A module whose one singleton logs its drop as `store dropped`, and whose before-shutdown hook
/// logs that it started and waits at its gate.
struct Gated {
    log: Log,
    hook_gate: Arc<Gate>,
}

impl Module for Gated {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Logged(self.log.clone(), "store dropped"));
        let (log, gate) = (self.log.clone(), Arc::clone(&self.hook_gate));
        m.before_shutdown(move |_signal: Signal| {
            let (log, gate) = (log.clone(), Arc::clone(&gate));
            async move {
                log.push("before-shutdown");
                gate.pass().await;
            }
        });
    }
}

/// An app binding two [`Recording`] servers, `a` then `b`, over a [`Gated`] root, with every gate
/// it waits at.
struct Paired {
    app: App<Serving>,
    log: Log,
    hook_gate: Arc<Gate>,
    drain_gate: Arc<Gate>,
    close_gate: Arc<Gate>,
}

/// `held` names the gates that start closed, of `hook`, `drain` and `close`.
async fn paired(held: &[&str]) -> Paired {
    let log = Log::default();
    let gate = |name| Gate::new(!held.contains(&name));
    let (hook_gate, drain_gate, close_gate) = (gate("hook"), gate("drain"), gate("close"));
    let server = |name| Recording {
        name,
        app: None,
        log: log.clone(),
        drain_gate: Arc::clone(&drain_gate),
        close_gate: Arc::clone(&close_gate),
    };
    let root = Gated { log: log.clone(), hook_gate: Arc::clone(&hook_gate) };
    let app = connect(App::builder(root).runtime(Tokio::current()))
        .await
        .bind(server("a"))
        .bind(server("b"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    Paired { app, log, hook_gate, drain_gate, close_gate }
}

/// Waits until `log` holds `event`, failing the test after [`PATIENCE`].
async fn until_logged(log: &Log, event: &str) {
    let deadline = Instant::now() + PATIENCE;
    while !log.has(event) {
        assert!(Instant::now() < deadline, "`{event}` was never logged: {:?}", log.events());
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// Runs `close` on a clone of `handle` in a task of its own.
fn close_elsewhere(handle: &AppHandle) -> tokio::task::JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>> {
    let handle = handle.clone();
    tokio::spawn(async move { handle.close(Signal::new("test")).await })
}

/// The servers' closes and drops so far, in order.
fn touched(log: &Log) -> Vec<String> {
    log.events().into_iter().filter(|e| e.starts_with("close ") || e.starts_with("drop ")).collect()
}

/// Neither server has closed or dropped yet.
fn assert_untouched(log: &Log, path: &str) {
    let touched = touched(log);
    assert!(touched.is_empty(), "a server closed or dropped at {path}: {touched:?}");
}

/// Every server's `close` started, in reverse bind order, before any server dropped; both servers
/// dropped once; and the store dropped last, with the last handle.
fn assert_closed_then_freed(log: &Log, path: &str) {
    let events = log.events();
    let closes: Vec<&str> = events.iter().filter(|e| e.starts_with("close ")).map(String::as_str).collect();
    assert_eq!(closes, ["close b", "close a"], "not every server's `close` ran after {path}: {events:?}");
    let first_drop = events.iter().position(|e| e.starts_with("drop ")).expect("no server dropped");
    let last_close = events.iter().rposition(|e| e.starts_with("close ")).expect("no server closed");
    assert!(last_close < first_drop, "a server dropped before every `close` ran after {path}: {events:?}");
    for drop in ["drop a", "drop b"] {
        assert_eq!(events.iter().filter(|e| *e == drop).count(), 1, "`{drop}` was not logged once after {path}: {events:?}");
    }
    assert_eq!(events.last().map(String::as_str), Some("store dropped"), "the store did not drop last after {path}: {events:?}");
}

/// The app's drop while a handle's `close` runs the shutdown in another task, inside the
/// before-shutdown hook, leaves the servers to that close: every server's `close` runs, then the
/// servers drop, then the store with the last handle.
#[tokio::test]
async fn an_app_dropped_before_a_running_close_s_close_step_leaves_it_every_server() {
    let Paired { app, log, hook_gate, close_gate, .. } = paired(&["hook", "close"]).await;
    let handle = app.handle();
    let closing = close_elsewhere(&handle);
    until_logged(&log, "before-shutdown").await;
    assert_eq!(handle.phase(), Phase::Stopping, "the close was not in its before-shutdown stage");
    drop(app);
    assert_untouched(&log, "the app's drop");
    hook_gate.open();
    // Both closes start at once; `b`'s waits at the gate, which holds the close step open while
    // the test reads it.
    until_logged(&log, "close a").await;
    assert_eq!(touched(&log), ["close b", "close a"], "the closes did not both start, last bound first");
    assert!(!log.has("store dropped"), "the store dropped while a server's `close` ran");
    close_gate.open();
    let outcome = tokio::time::timeout(PATIENCE, closing).await.expect("the close did not end").expect("the close task panicked");
    outcome.expect("the close failed");
    assert!(!log.has("store dropped"), "the store dropped while a handle was held");
    drop(handle);
    assert_closed_then_freed(&log, "a drop inside the before-shutdown hook");
}

/// The same, with the app dropped during the drain, both servers' `drain` waiting at its gate.
#[tokio::test]
async fn an_app_dropped_during_a_running_close_s_drain_leaves_it_every_server() {
    let Paired { app, log, drain_gate, .. } = paired(&["drain"]).await;
    let handle = app.handle();
    let closing = close_elsewhere(&handle);
    until_logged(&log, "drain a").await;
    until_logged(&log, "drain b").await;
    assert_eq!(handle.phase(), Phase::Draining, "the close was not in its drain");
    drop(app);
    assert_untouched(&log, "the app's drop");
    drain_gate.open();
    let outcome = tokio::time::timeout(PATIENCE, closing).await.expect("the close did not end").expect("the close task panicked");
    outcome.expect("the close failed");
    assert!(!log.has("store dropped"), "the store dropped while a handle was held");
    drop(handle);
    assert_closed_then_freed(&log, "a drop during the drain");
}

/// A handle's `close` the app's drop left the servers to, itself dropped inside the before-shutdown
/// hook and never resumed, releases them as it drops: each server drops with its `close` not run,
/// and the store drops with the last handle.
#[tokio::test]
async fn a_close_abandoned_after_the_app_s_drop_releases_the_servers() {
    let Paired { app, log, .. } = paired(&["hook"]).await;
    let handle = app.handle();
    let closing = close_elsewhere(&handle);
    until_logged(&log, "before-shutdown").await;
    drop(app);
    assert_untouched(&log, "the app's drop");
    closing.abort();
    let ended = tokio::time::timeout(PATIENCE, closing).await.expect("the aborted close did not end");
    assert!(ended.is_err_and(|error| error.is_cancelled()), "the close was not abandoned");
    let events = log.events();
    for drop in ["drop a", "drop b"] {
        assert_eq!(events.iter().filter(|e| *e == drop).count(), 1, "`{drop}` was not logged once after the abandoned close: {events:?}");
    }
    assert!(!events.iter().any(|e| e.starts_with("close ")), "a server's `close` ran after the abandoned close: {events:?}");
    assert!(!log.has("store dropped"), "the store dropped while a handle was held");
    drop(handle);
    assert_eq!(log.events().last().map(String::as_str), Some("store dropped"), "the store outlived the last handle");
}

/// A `close` called before the app's drop and first polled after it holds its claim from the call:
/// the drop leaves it the servers, and its poll runs the whole shutdown, the before-shutdown hook,
/// every server's `close`, then the servers' drops and the store's with the last handle.
#[tokio::test]
async fn a_close_called_before_the_app_s_drop_and_polled_after_it_closes_every_server() {
    let Paired { app, log, .. } = paired(&[]).await;
    let handle = app.handle();
    let closing = handle.close(Signal::new("test"));
    drop(app);
    assert_untouched(&log, "the app's drop");
    assert!(!log.has("before-shutdown"), "the shutdown started before the close was polled");
    let outcome = tokio::time::timeout(PATIENCE, closing).await.expect("the close did not end");
    outcome.expect("the close failed");
    assert!(log.has("before-shutdown"), "the shutdown's hook did not run: {:?}", log.events());
    assert_eq!(handle.phase(), Phase::Closed, "the shutdown did not run to its end");
    assert!(!log.has("store dropped"), "the store dropped while a handle was held");
    drop(handle);
    assert_closed_then_freed(&log, "a close polled after the app's drop");
}

/// A `close` called before the app's drop and dropped unpolled after it releases the servers as it
/// drops, their `close` not run, and the store drops with the last handle.
#[tokio::test]
async fn a_close_called_before_the_app_s_drop_and_dropped_unpolled_releases_the_servers() {
    let Paired { app, log, .. } = paired(&[]).await;
    let handle = app.handle();
    let closing = handle.close(Signal::new("test"));
    drop(app);
    assert_untouched(&log, "the app's drop");
    drop(closing);
    let events = log.events();
    for drop in ["drop a", "drop b"] {
        assert_eq!(events.iter().filter(|e| *e == drop).count(), 1, "`{drop}` was not logged once after the unpolled close's drop: {events:?}");
    }
    assert!(!events.iter().any(|e| e.starts_with("close ") || e == "before-shutdown"), "the unpolled close ran a step: {events:?}");
    assert!(!log.has("store dropped"), "the store dropped while a handle was held");
    drop(handle);
    assert_eq!(log.events().last().map(String::as_str), Some("store dropped"), "the store outlived the last handle");
}

/// A `close` called after the app's drop released the servers answers `Dropped` and runs no hook.
#[tokio::test]
async fn a_close_called_after_the_app_s_drop_answers_that_the_app_was_dropped() {
    let Paired { app, log, .. } = paired(&[]).await;
    let handle = app.handle();
    drop(app);
    let error = handle.close(Signal::new("test")).await.expect_err("a close after the app's drop answered `Ok`");
    assert!(matches!(&*error.failures, [ShutdownFailure::Dropped]), "the close answered {error}");
    assert!(error.to_string().contains("the app was dropped before its shutdown closed its servers"), "the close answered {error}");
    let events = log.events();
    assert!(!events.iter().any(|e| e.starts_with("close ") || e == "before-shutdown"), "the close ran a step: {events:?}");
    assert!(handle.close(Signal::new("again")).await.is_err_and(|again| matches!(&*again.failures, [ShutdownFailure::Dropped])), "a second close did not answer `Dropped`");
    drop(handle);
    assert_eq!(log.events().last().map(String::as_str), Some("store dropped"), "the store outlived the last handle");
}

/// A second `close` called after the drop while the first, which answers `Dropped`, still holds its
/// claim finds the servers released too: it answers `Dropped` and runs no hook.
#[tokio::test]
async fn a_close_called_after_the_drop_beside_another_answers_that_the_app_was_dropped() {
    let Paired { app, log, .. } = paired(&[]).await;
    let handle = app.handle();
    drop(app);
    let first = handle.close(Signal::new("first"));
    let second = handle.close(Signal::new("second"));
    let error = second.await.expect_err("a close beside another after the app's drop answered `Ok`");
    assert!(matches!(&*error.failures, [ShutdownFailure::Dropped]), "the close answered {error}");
    assert!(first.await.is_err_and(|first| matches!(&*first.failures, [ShutdownFailure::Dropped])), "the first close did not answer `Dropped`");
    assert!(!log.has("before-shutdown"), "a close after the app's drop ran a hook: {:?}", log.events());
}

/// The first test's case raced on two workers: a `close` spawned before the app's drop, with no
/// wait between the spawn and the drop. Every race closes every server. The races in which the
/// phase still reads `Running` after the drop are ones the drop won, its shutdown not yet
/// triggered; the test requires at least one, so it exercises the case it is named for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_close_spawned_before_the_app_s_drop_closes_every_server_in_every_race() {
    let mut drop_first = 0;
    for race in 0..RACES {
        let Paired { app, log, .. } = paired(&[]).await;
        let handle = app.handle();
        let closing = tokio::spawn(handle.close(Signal::new("test")));
        drop(app);
        if handle.phase() == Phase::Running {
            drop_first += 1;
        }
        let outcome = tokio::time::timeout(PATIENCE, closing).await.expect("the close did not end").expect("the close task panicked");
        outcome.unwrap_or_else(|error| panic!("race {race}: the close failed: {error}"));
        drop(handle);
        assert_closed_then_freed(&log, &format!("race {race}"));
    }
    println!("{RACES} races, every server closed in each; the drop ran before the trigger in {drop_first}");
    assert!(drop_first > 0, "the drop ran before the trigger in none of {RACES} races");
}

/// How many races the race test runs.
const RACES: usize = 500;

/// A loop holding a handle waits on `draining()` until the app's drop releases the servers.
fn wait_for_draining(handle: &AppHandle) -> tokio::task::JoinHandle<()> {
    let handle = handle.clone();
    tokio::spawn(async move { handle.draining().await })
}

/// The loop is still waiting and the app still running a moment after it started to wait.
async fn assert_still_waiting(waiting: &tokio::task::JoinHandle<()>, handle: &AppHandle, path: &str) {
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiting.is_finished(), "`draining()` resolved before {path}");
    assert_eq!(handle.phase(), Phase::Running, "the phase moved before {path}");
}

/// After a release by a drop: the loop has woken, the phase reads `Closed`, neither server's
/// `close` ran, and a later `close` answers `Dropped`.
async fn assert_told_to_stop(waiting: tokio::task::JoinHandle<()>, handle: &AppHandle, log: &Log, path: &str) {
    tokio::time::timeout(PATIENCE, waiting)
        .await
        .unwrap_or_else(|_| panic!("`draining()` did not resolve after {path}"))
        .expect("the waiting task panicked");
    assert_eq!(handle.phase(), Phase::Closed, "the phase after {path}");
    assert!(handle.is_draining(), "`is_draining()` is false after {path}");
    let events = log.events();
    assert!(!events.iter().any(|e| e.starts_with("close ") || e == "before-shutdown"), "a shutdown step ran after {path}: {events:?}");
    for drop in ["drop a", "drop b"] {
        assert_eq!(events.iter().filter(|e| *e == drop).count(), 1, "`{drop}` was not logged once after {path}: {events:?}");
    }
    let error = handle.close(Signal::new("later")).await.expect_err("a close after the release answered `Ok`");
    assert!(matches!(&*error.failures, [ShutdownFailure::Dropped]), "a close after {path} answered {error}");
}

/// The `App` value's drop with no `close` claim held releases the servers unclosed, and that
/// release tells a loop waiting on `draining()` to stop: it wakes, and `phase()` reads `Closed`.
#[tokio::test]
async fn a_loop_waiting_on_draining_wakes_when_the_app_is_dropped() {
    let Paired { app, log, .. } = paired(&[]).await;
    let handle = app.handle();
    let waiting = wait_for_draining(&handle);
    assert_still_waiting(&waiting, &handle, "the app's drop").await;
    drop(app);
    assert_told_to_stop(waiting, &handle, &log, "the app's drop").await;
}

/// The same when the release is the last claim's: a `close` called before the app's drop and
/// dropped unpolled after it. The app's drop leaves the loop waiting, and the claim's drop wakes it.
#[tokio::test]
async fn a_loop_waiting_on_draining_wakes_when_the_last_claim_releases_the_servers() {
    let Paired { app, log, .. } = paired(&[]).await;
    let handle = app.handle();
    let waiting = wait_for_draining(&handle);
    let closing = handle.close(Signal::new("test"));
    drop(app);
    assert_still_waiting(&waiting, &handle, "the claim's drop").await;
    drop(closing);
    assert_told_to_stop(waiting, &handle, &log, "the claim's drop").await;
}

/// A second transport, so the failure names the one whose `serve` ended.
struct Doomed;

impl Transport for Doomed {
    const KEY: &'static str = "doomed";
    type Cx = ();
    type Reply = ();
}

/// The error a [`Dying`] server's `serve` returns.
#[derive(Debug)]
struct ListenerLost;

impl std::fmt::Display for ListenerLost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the listener was lost")
    }
}

impl std::error::Error for ListenerLost {}

/// A server whose `serve` waits at its gate, then ends with `fails`' answer: `Err(ListenerLost)`
/// when set, `Ok` otherwise. Its `drain` and `close` are logged as `drain dying` and `close dying`.
struct Dying {
    log: Log,
    gate: Arc<Gate>,
    fails: bool,
}

impl Server for Dying {
    type Transport = Doomed;

    async fn bind(&mut self, _mounted: Mounted<'_, Doomed>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.gate.pass().await;
        if self.fails { Err(Box::new(ListenerLost)) } else { Ok(()) }
    }

    async fn drain(&self, _token: DrainToken) {
        self.log.push("drain dying");
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.log.push("close dying");
        Ok(())
    }
}

/// An app serving a [`Recording`] server `a` and a [`Dying`] one bound after it. Once `serve` runs
/// with no trigger, the dying server's `serve` ends: the shutdown runs as for any trigger, the
/// hook, both drains and both closes in reverse bind order, the phase reaches `Closed`, and `serve`
/// answers `Err` whose one failure is `Serve` naming the dying transport, its `end` checked by
/// `ended`. A later `close` answers the same failure.
async fn a_serve_that_ends(fails: bool, ended: impl Fn(&ServeEnd) -> bool) {
    let log = Log::default();
    let gate = Gate::new(false);
    let recording = Recording { name: "a", app: None, log: log.clone(), drain_gate: Gate::new(true), close_gate: Gate::new(true) };
    let root = Gated { log: log.clone(), hook_gate: Gate::new(true) };
    let app = connect(App::builder(root).runtime(Tokio::current()))
        .await
        .bind(recording)
        .bind(Dying { log: log.clone(), gate: Arc::clone(&gate), fails })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(handle.phase(), Phase::Running, "the app stopped running before the dying server's `serve` ended");
    assert!(log.events().is_empty(), "a shutdown step ran before the dying server's `serve` ended: {:?}", log.events());
    gate.open();
    let outcome = tokio::time::timeout(PATIENCE, serving).await.expect("`serve` did not end").expect("the serving task panicked");
    let error = outcome.expect_err("`serve` answered `Ok` after a transport's `serve` ended");
    let events = log.events();
    let steps: Vec<&str> = events.iter().map(String::as_str).filter(|e| !e.starts_with("drop ") && *e != "store dropped").collect();
    assert_eq!(steps[0], "before-shutdown", "the before-shutdown hook did not run first: {events:?}");
    let mut drains = steps[1..3].to_vec();
    drains.sort_unstable();
    assert_eq!(drains, ["drain a", "drain dying"], "not every server drained: {events:?}");
    assert_eq!(&steps[3..], ["close dying", "close a"], "not every server closed, in reverse bind order: {events:?}");
    assert_eq!(handle.phase(), Phase::Closed, "the shutdown did not run to its end");
    let [ShutdownFailure::Serve { transport, end }] = &*error.failures else {
        panic!("`serve` did not answer the one `Serve` failure: {error}");
    };
    assert_eq!(*transport, TypeName::of::<Doomed>(), "the failure names another transport");
    assert!(ended(end), "the failure carries another end: {end:?}");
    let again = handle.close(Signal::new("again")).await.expect_err("a close after the shutdown answered `Ok`");
    assert!(matches!(&*again.failures, [ShutdownFailure::Serve { .. }]), "a close after the shutdown answered {again}");
}

#[tokio::test]
async fn a_transport_whose_serve_fails_closes_the_app() {
    a_serve_that_ends(true, |end| matches!(end, ServeEnd::Errored(reason) if reason.downcast_ref::<ListenerLost>().is_some())).await;
}

#[tokio::test]
async fn a_transport_whose_serve_returns_while_the_app_runs_closes_the_app() {
    a_serve_that_ends(false, |end| matches!(end, ServeEnd::Returned)).await;
}

/// A second doomed transport, so two [`Dying`] servers name different transports.
struct Fated;

impl Transport for Fated {
    const KEY: &'static str = "fated";
    type Cx = ();
    type Reply = ();
}

/// A [`Dying`] server bound under transport `T`.
struct DyingAs<T>(Dying, std::marker::PhantomData<fn() -> T>);

impl<T: Transport> Server for DyingAs<T> {
    type Transport = T;

    async fn bind(&mut self, _mounted: Mounted<'_, T>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        Server::serve(&self.0).await
    }

    async fn drain(&self, token: DrainToken) {
        Server::drain(&self.0, token).await;
    }

    async fn close(&self) -> Result<(), BoxError> {
        Server::close(&self.0).await
    }
}

/// A module recording the signal each shutdown hook receives, as `before-shutdown <name>` and
/// `on-shutdown <name>`.
struct Signalled(Log);

impl Module for Signalled {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let log = self.0.clone();
        m.before_shutdown(move |signal: Signal| {
            let log = log.clone();
            async move { log.push(format!("before-shutdown {}", signal.name())) }
        });
        let log = self.0.clone();
        m.on_shutdown(move |signal: Signal| {
            let log = log.clone();
            async move { log.push(format!("on-shutdown {}", signal.name())) }
        });
    }
}

/// Two servers whose `serve` ends on the same poll, both failing: the first bound is the cause,
/// listed once, the second's end is part of that shutdown, and both hooks receive the signal the
/// outcome reports, which names the first. Run several times, the answer being the same each time.
#[tokio::test]
async fn two_serves_ending_on_one_poll_record_the_first_bound_once() {
    for run in 0..20 {
        let log = Log::default();
        let gate = Gate::new(false);
        let dying = |fails| Dying { log: log.clone(), gate: Arc::clone(&gate), fails };
        let app = connect(App::builder(Signalled(log.clone())).runtime(Tokio::current()))
            .await
            .bind(DyingAs::<Doomed>(dying(true), std::marker::PhantomData))
            .bind(DyingAs::<Fated>(dying(true), std::marker::PhantomData))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
        let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
        tokio::time::sleep(Duration::from_millis(5)).await;
        gate.open();
        let outcome = tokio::time::timeout(PATIENCE, serving).await.expect("`serve` did not end").expect("the serving task panicked");
        let error = outcome.expect_err("`serve` answered `Ok` after two transports' `serve` failed");
        let [ShutdownFailure::Serve { transport, end: ServeEnd::Errored(_) }] = &*error.failures else {
            panic!("run {run}: `serve` did not answer one `Serve` failure: {:?}", error.failures);
        };
        assert_eq!(*transport, TypeName::of::<Doomed>(), "run {run}: the cause is not the first server bound");
        let signal = error.report.signal.name().to_owned();
        let hooks: Vec<String> = log.events().into_iter().filter(|e| e.contains("shutdown ")).collect();
        assert_eq!(hooks, [format!("before-shutdown {signal}"), format!("on-shutdown {signal}")], "run {run}: the hooks' signals");
    }
}

/// A `serve` that fails once a `close` has triggered the shutdown is part of that shutdown: the
/// outcome is `Ok` under the close's signal, for `serve` and the `close` alike.
#[tokio::test]
async fn a_serve_failing_after_a_close_is_no_failure() {
    let log = Log::default();
    let (gate, hook_gate) = (Gate::new(false), Gate::new(false));
    let app = connect(App::builder(Gated { log: log.clone(), hook_gate: Arc::clone(&hook_gate) }).runtime(Tokio::current()))
        .await
        .bind(Dying { log: log.clone(), gate: Arc::clone(&gate), fails: true })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    tokio::time::sleep(Duration::from_millis(5)).await;
    let closing = tokio::spawn(handle.close(Signal::new("operator")));
    let deadline = Instant::now() + PATIENCE;
    while !log.has("before-shutdown") {
        assert!(Instant::now() < deadline, "the before-shutdown hook did not start");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    gate.open();
    tokio::time::sleep(Duration::from_millis(20)).await;
    hook_gate.open();
    let served = tokio::time::timeout(PATIENCE, serving).await.expect("`serve` did not end").expect("the serving task panicked");
    let closed = tokio::time::timeout(PATIENCE, closing).await.expect("`close` did not end").expect("the closing task panicked");
    for (who, outcome) in [("serve", served), ("close", closed)] {
        let report = outcome.unwrap_or_else(|error| panic!("`{who}` answered {:?}", error.failures));
        assert_eq!(report.signal.name(), "operator", "`{who}` reported another trigger");
    }
}

/// What a [`Reentrant`] server's drop saw: the phase and whether `draining()` had fired, both read
/// as the drop began, and whether the `close` it called answered `Dropped` on its first poll.
type SeenInDrop = Arc<Mutex<Option<(Phase, bool, bool)>>>;

/// A server keeping its `AppHandle` whose own drop calls `close` on it and polls it once.
struct Reentrant {
    app: Option<AppHandle>,
    seen: SeenInDrop,
}

impl Drop for Reentrant {
    fn drop(&mut self) {
        let Some(app) = self.app.take() else { return };
        // Read before the claim: the claim's own drop, at the end of its first poll, releases again.
        let (phase, draining) = (app.phase(), app.is_draining());
        let mut closing = std::pin::pin!(app.close(Signal::new("from the drop")));
        let polled = closing.as_mut().poll(&mut std::task::Context::from_waker(Waker::noop()));
        let dropped = matches!(&polled, Poll::Ready(Err(error)) if matches!(&*error.failures, [ShutdownFailure::Dropped]));
        *self.seen.lock().unwrap_or_else(PoisonError::into_inner) = Some((phase, draining, dropped));
    }
}

impl Server for Reentrant {
    type Transport = Bare;

    async fn prepare(&mut self, mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        self.app = Some(mounted.app().clone());
        Ok(())
    }

    async fn bind(&mut self, _mounted: Mounted<'_, Bare>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// A server whose drop, run by the `App` value's drop, takes a `close` claim and drops it: the
/// phase already reads `Closed` and `draining()` has fired as the server drops, the claim answers
/// `Dropped` at once, and neither the claim nor its drop waits on the release under way.
#[tokio::test]
async fn a_close_inside_a_server_s_drop_answers_dropped() {
    let seen = SeenInDrop::default();
    let app = connect(App::builder(Gated { log: Log::default(), hook_gate: Gate::new(true) }).runtime(Tokio::current()))
        .await
        .bind(Reentrant { app: None, seen: Arc::clone(&seen) })
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not bind: {error}"));
    drop(app);
    let seen = seen.lock().unwrap_or_else(PoisonError::into_inner).take();
    assert_eq!(seen, Some((Phase::Closed, true, true)), "what the server's drop saw: (phase, draining, `Dropped`)");
}
