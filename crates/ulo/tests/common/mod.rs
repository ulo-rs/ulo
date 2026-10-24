//! Shared harness for the core test series (`CORE_TEST_PLAN.md` section 0).
//!
//! Every integration test file declares `#[macro_use] mod common;` and uses the value-level API
//! only; no macro from `ulo-macros` appears here. Not every file uses every helper, so unused
//! items are expected and allowed.
//!
//! # What is here
//!
//! - [`Empty`]: a module that registers nothing.
//! - [`Mod<K>`]: a module of a type of its own for each marker `K`, built from a closure. Two
//!   `Mod<A>` and `Mod<B>` are two modules; `Mod<A>` imported twice is one. A module prints as
//!   `Mod<A>`, with `A` cut to its last path segment; [`mod_name`] writes that text for assertions.
//! - [`wire_err`], [`wire_err_with`], [`wiring_errors`]: make `wire()` fail and hand back the
//!   [`WiringErrors`]; they panic with the real outcome when `wire()` succeeds or fails another way.
//! - [`wires`]: the opposite, `wire()` must succeed.
//! - [`entry!`] and [`entries!`]: pick the entries of one variant out of a report and destructure
//!   them in one expression, so an assertion names variant and fields and not message text.
//! - [`variants`]: the variant name of each entry, in report order (steps 1 to 6 of section 10.1).
//! - [`Probe`]: records the order in which fixtures were constructed or hooks ran.
//! - [`Frozen`]: a `Timer` whose every sleep ends at once, so an app has a clock without a runtime.
//! - [`TA`], [`TB`], [`GuardA`]: two test transports and a guard for one of them, for role keys.
//! - [`unit_service!`]: unit structs the container can build, for `provide::<S>()`.
//! - [`watchdog`]: aborts the test process if it runs past 60 s, so a hang is a failure and never a stall.
//!   Every wiring helper starts it.
//! - [`Boom`]: an error type with a message, for failing factories and `try_value`.
//!
//! Tests that need a runtime (hooks, constructor waits) use `#[tokio::test]` with
//! `ulo_runtime_tokio::Tokio::current()`; B1 needs none, since wiring is synchronous.

// `StartupError` is large by design; the helpers return it as `wire()` does.
#![allow(dead_code, unused_macros, clippy::result_large_err)]

use std::error::Error;
use std::fmt;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use ulo::{
    App, AppBuilder, BoxError, BoxFuture, Guard, Module, ModuleDef, ModuleIdentity, StartupError, Timer, Transport, TypeName,
    Wired, WiringError, WiringErrors,
};

/// A module that registers nothing.
pub struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// A module whose registration is a closure and whose type is `Mod<K>`. Declare a marker per
/// module (`struct Billing;`) and write `Mod::<Billing>::new(|m| { .. })`; the marker is the
/// module's identity and its printed name.
pub struct Mod<K: 'static> {
    register: Box<dyn Fn(&mut ModuleDef<'_>) + Send + Sync>,
    _k: PhantomData<fn() -> K>,
}

impl<K: 'static> Mod<K> {
    pub fn new(register: impl Fn(&mut ModuleDef<'_>) + Send + Sync + 'static) -> Self {
        Mod { register: Box::new(register), _k: PhantomData }
    }

    /// `Mod<K>` registering nothing.
    pub fn bare() -> Self {
        Mod::new(|_| {})
    }
}

impl<K: 'static> Module for Mod<K> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        (self.register)(m);
    }
}

/// How a report prints `Mod<K>`: `Mod<K>` with `K` cut to its last path segment.
pub fn mod_name<K: 'static>() -> String {
    format!("Mod<{}>", TypeName::of::<K>())
}

/// How a keyed `Mod<K>` prints: `Mod<K> @ Q`.
pub fn keyed_mod_name<K: 'static, Q: 'static>() -> String {
    format!("{} @ {}", mod_name::<K>(), TypeName::of::<Q>())
}

/// Starts, once per test process, a thread that aborts the process 60 seconds in. The suite takes
/// well under a second, so reaching the deadline means a test hung.
pub fn watchdog() {
    static STARTED: std::sync::Once = std::sync::Once::new();
    STARTED.call_once(|| {
        std::thread::spawn(|| {
            std::thread::sleep(Duration::from_secs(60));
            eprintln!("watchdog: the test process ran past 60 s; a test is hung");
            std::process::abort();
        });
    });
}

/// `wire()` on `root` with the default builder, which has no `Timer`.
pub fn wire_result(root: impl Module) -> Result<App<Wired>, StartupError> {
    watchdog();
    App::builder(root).wire()
}

/// The report of a `wire()` that must fail with wiring errors.
pub fn wire_err(root: impl Module) -> WiringErrors {
    wiring_errors(wire_result(root))
}

/// As [`wire_err`], for a builder the test configured (a timer, a knob).
pub fn wire_err_with(builder: AppBuilder) -> WiringErrors {
    watchdog();
    wiring_errors(builder.wire())
}

/// The report inside a `wire()` outcome. Panics, saying what happened instead, on success and on a
/// startup error that is not `Wiring`.
pub fn wiring_errors(outcome: Result<App<Wired>, StartupError>) -> WiringErrors {
    match outcome {
        Err(StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("wire() failed, but not with wiring errors: {other}"),
        Ok(_) => panic!("wire() succeeded; the graph was expected to be refused"),
    }
}

/// The wired app of a `wire()` that must succeed; the report is the panic message otherwise.
pub fn wires(root: impl Module) -> App<Wired> {
    wires_with(App::builder(root))
}

pub fn wires_with(builder: AppBuilder) -> App<Wired> {
    watchdog();
    match builder.wire() {
        Ok(app) => app,
        Err(error) => panic!("wire() refused a graph that should wire:\n{error}"),
    }
}

/// The variant name of every entry, in report order. The report lists entries by step, so this is
/// also the order of section 10.1.
pub fn variants(errors: &WiringErrors) -> Vec<String> {
    errors.iter().map(variant).collect()
}

/// `Missing` for `WiringError::Missing { .. }`: the derived `Debug` opens with the variant name.
pub fn variant(error: &WiringError) -> String {
    format!("{error:?}").split([' ', '{', '(']).next().unwrap_or_default().to_owned()
}

/// Every entry of `$errors` matching `$pat`, as a `Vec` of `$out`. `$pat` binds fields by
/// reference.
macro_rules! entries {
    ($errors:expr, $pat:pat => $out:expr) => {{
        let errors: &ulo::WiringErrors = &$errors;
        errors
            .iter()
            .filter_map(|entry| match entry {
                $pat => Some($out),
                _ => None,
            })
            .collect::<Vec<_>>()
    }};
}

/// The one entry of `$errors` matching `$pat`, as `$out`. Fails the test, printing the whole
/// report, when no entry or several match.
macro_rules! entry {
    ($errors:expr, $pat:pat => $out:expr) => {{
        let mut found = entries!($errors, $pat => $out);
        assert!(
            found.len() == 1,
            "expected exactly one `{}` entry, found {}; the report:\n{}",
            stringify!($pat),
            found.len(),
            $errors
        );
        found.remove(0)
    }};
}

/// An error with a message, `Send + Sync` and `std::error::Error`.
#[derive(Debug)]
pub struct Boom(pub String);

impl Boom {
    pub fn new(text: impl Into<String>) -> Self {
        Boom(text.into())
    }
}

impl fmt::Display for Boom {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for Boom {}

/// Records the order in which fixtures were constructed or hooks ran.
#[derive(Clone, Default)]
pub struct Probe(Arc<Mutex<Vec<&'static str>>>);

impl Probe {
    pub fn new() -> Self {
        Probe::default()
    }

    pub fn mark(&self, what: &'static str) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(what);
    }

    pub fn log(&self) -> Vec<&'static str> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn is_empty(&self) -> bool {
        self.log().is_empty()
    }
}

/// A clock that stands still a day ahead and ends every sleep at once. It gives an app a `Timer`
/// without a runtime.
pub struct Frozen(Instant);

impl Frozen {
    pub fn new() -> Self {
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

/// Unit structs the container can build, so a test can `m.provide::<S>()` without a macro.
macro_rules! unit_service {
    ($($name:ident),+ $(,)?) => {$(
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = ulo::scope::Singleton;

            fn dependencies(_d: &mut ulo::Dependencies) {}

            async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                Ok($name)
            }
        }
    )+};
}

/// Two transports with unit contexts; their role keys are `AnyGuard<TA>` and `AnyGuard<TB>`.
pub struct TA;

impl Transport for TA {
    const KEY: &'static str = "a";
    type Cx = ();
    type Reply = ();
}

pub struct TB;

impl Transport for TB {
    const KEY: &'static str = "b";
    type Cx = ();
    type Reply = ();
}

/// A guard for `TA` that admits everything.
pub struct GuardA;

impl Guard<TA> for GuardA {
    async fn can_activate(&self, _cx: &()) -> Result<bool, BoxError> {
        Ok(true)
    }
}

impl ulo::Construct for GuardA {
    type Scope = ulo::scope::Singleton;

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(GuardA)
    }
}

// ---- Appended by batch B3 (scopes, executions, lookups, overrides). Nothing above changed. ----

/// A shared construction counter: clone it into a factory, `bump` where the instance is built.
#[derive(Clone, Default)]
pub struct Count(Arc<std::sync::atomic::AtomicUsize>);

impl Count {
    pub fn new() -> Self {
        Count::default()
    }

    /// Adds one and returns the new total.
    pub fn bump(&self) -> usize {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    pub fn get(&self) -> usize {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// A `Construct` unit struct of scope `$scope` whose injection points are the listed fields: it
/// declares each to the wiring pass and reads each when built. `reader!(Name, Scope, f: Ty, ..)`.
macro_rules! reader {
    ($name:ident, $scope:ty $(, $field:ident : $ty:ty)* $(,)?) => {
        pub struct $name;

        impl ulo::Construct for $name {
            type Scope = $scope;

            fn dependencies(d: &mut ulo::Dependencies) {
                $( d.field::<$ty>(stringify!($field)); )*
                let _ = d;
            }

            async fn construct(r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
                $( let _ = <$ty as ulo::FromContainer>::from_container(r).await?; )*
                let _ = r;
                Ok($name)
            }
        }
    };
}

/// A [`reader!`] that is also a `Controller`; `mount` is the given body, so a test mounts the
/// handlers it needs: `controller!(C, Auto, [u: Ext<U>], |m| m.handler::<TA, _>(..))`.
macro_rules! controller {
    ($name:ident, $scope:ty, [$($field:ident : $ty:ty),*], |$m:ident| $body:expr) => {
        reader!($name, $scope $(, $field : $ty)*);

        impl ulo::Controller for $name {
            fn mount($m: &mut ulo::Mount<'_>) {
                $body
            }
        }
    };
}

// Appended by batch B2: connect and qualified-read helpers.

/// `wire()` then `connect()` on `root` with the default builder (no `Timer`, no runtime). Panics
/// with the startup error when either step fails.
pub async fn connects(root: impl Module) -> ulo::App<ulo::Connected> {
    watchdog();
    match App::builder(root).wire() {
        Ok(app) => match app.connect().await {
            Ok(app) => app,
            Err(error) => panic!("connect() failed on a graph that should connect:\n{error}"),
        },
        Err(error) => panic!("wire() refused a graph that should wire:\n{error}"),
    }
}

/// `connect()` on a wired app and the startup error it must fail with.
pub async fn connect_err(root: impl Module) -> StartupError {
    watchdog();
    match wires(root).connect().await {
        Err(error) => error,
        Ok(_) => panic!("connect() succeeded; the graph was expected to fail while connecting"),
    }
}

/// `T @ Q` as the root module sees it. `App::get` reads the unqualified key only, so a qualified
/// key is read through a standalone execution's resolver.
pub async fn read_q<T: ?Sized + Send + Sync + 'static, Q: 'static>(
    app: &ulo::App<ulo::Connected>,
) -> Result<ulo::Dep<T, Q>, ulo::LookupError> {
    app.execute(ulo::ExecOptions::new(), async |exec| exec.resolver().dep_qualified::<T, Q>().await)
        .await
        .expect("a connected app opens executions")
}

/// Every contribution to `T` as the root sees them, in collection order.
pub async fn read_many<T: ?Sized + Send + Sync + 'static>(
    app: &ulo::App<ulo::Connected>,
) -> Result<ulo::Many<T>, ulo::LookupError> {
    app.execute(ulo::ExecOptions::new(), async |exec| exec.resolver().many::<T>().await)
        .await
        .expect("a connected app opens executions")
}

/// Wires and connects `root` on an app with no clock and no runtime; panics with the error
/// otherwise. Runs under [`watchdog`].
pub async fn connected(root: impl Module) -> ulo::App<ulo::Connected> {
    connected_with(App::builder(root)).await
}

pub async fn connected_with(builder: AppBuilder) -> ulo::App<ulo::Connected> {
    watchdog();
    match builder.wire() {
        Ok(app) => app.connect().await.unwrap_or_else(|error| panic!("connect() failed:\n{error}")),
        Err(error) => panic!("wire() refused a graph that should wire:\n{error}"),
    }
}

/// As [`connected`], on the tokio runtime of the calling `#[tokio::test]`, which a test that
/// asks the app for its mounted handlers needs.
pub async fn connected_on_tokio(root: impl Module) -> ulo::App<ulo::Connected> {
    connected_with(App::builder(root).runtime(ulo_runtime_tokio::Tokio::current())).await
}

/// Closes a connected app, panicking if the shutdown reports a failure.
pub async fn close(app: ulo::App<ulo::Connected>) {
    app.close(ulo::Signal::new("test")).await.unwrap_or_else(|error| panic!("close() failed: {error:?}"));
}

// ---- Appended by batch B4 (hooks, readiness, phases, lazy loading, shutdown). Nothing above changed. ----

/// What happened, in order, with owned text: a [`Probe`] whose entries are built at run time.
#[derive(Clone, Default)]
pub struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    pub fn new() -> Self {
        Log::default()
    }

    pub fn push(&self, event: impl Into<String>) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).push(event.into());
    }

    pub fn events(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    pub fn has(&self, event: &str) -> bool {
        self.events().iter().any(|seen| seen == event)
    }

    pub fn count(&self, event: &str) -> usize {
        self.events().iter().filter(|seen| *seen == event).count()
    }
}

/// A gate a future waits at until the test opens it.
pub struct Gate(Mutex<(bool, Vec<std::task::Waker>)>);

impl Gate {
    pub fn new(open: bool) -> Arc<Gate> {
        Arc::new(Gate(Mutex::new((open, Vec::new()))))
    }

    pub fn open(&self) {
        let waiting = {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            state.0 = true;
            std::mem::take(&mut state.1)
        };
        waiting.into_iter().for_each(std::task::Waker::wake);
    }

    pub async fn pass(&self) {
        std::future::poll_fn(|cx| {
            let mut state = self.0.lock().unwrap_or_else(PoisonError::into_inner);
            if state.0 {
                return std::task::Poll::Ready(());
            }
            state.1.push(cx.waker().clone());
            std::task::Poll::Pending
        })
        .await
    }
}

/// Every duration a [`Recorded`] timer was asked to sleep, in order.
#[derive(Clone, Default)]
pub struct SleepLog(Arc<Mutex<Vec<Duration>>>);

impl SleepLog {
    pub fn new() -> Self {
        SleepLog::default()
    }

    pub fn asked(&self) -> Vec<Duration> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// How many sleeps of exactly `d` were asked for.
    pub fn count(&self, d: Duration) -> usize {
        self.asked().into_iter().filter(|asked| *asked == d).count()
    }
}

/// [`Frozen`] that writes down each sleep it is asked for.
pub struct Recorded(Instant, SleepLog);

impl Recorded {
    pub fn new(log: &SleepLog) -> Self {
        Recorded(Instant::now() + Duration::from_secs(86_400), log.clone())
    }
}

impl Timer for Recorded {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.1.0.lock().unwrap_or_else(PoisonError::into_inner).push(d);
        Box::pin(async {})
    }

    fn now(&self) -> Instant {
        self.0
    }
}

/// How a [`TestServer`]'s `close` ends.
#[derive(Clone, Copy)]
pub enum CloseMode {
    Ok,
    Err,
    Panic,
    /// Never completes.
    Pending,
}

/// What a [`TestServer`] does with the drain: called with the app's handle and the token the core
/// hands each server.
pub type OnDrain = Arc<dyn Fn(ulo::AppHandle, ulo::DrainToken) -> BoxFuture<'static, ()> + Send + Sync>;

/// A server for transport `T` that binds nothing. It keeps the `AppHandle` its `prepare` is handed,
/// as every shipped transport does, and logs `prepare`, `bind`, `drain` and `close` as
/// `<verb> <name>`. Its `serve` never ends.
pub struct TestServer<T> {
    name: &'static str,
    log: Log,
    app: Option<ulo::AppHandle>,
    fail_prepare: bool,
    fail_bind: bool,
    permanent: bool,
    close: CloseMode,
    on_drain: Option<OnDrain>,
    _t: PhantomData<fn() -> T>,
}

impl<T: Transport> TestServer<T> {
    pub fn new(name: &'static str, log: &Log) -> Self {
        TestServer {
            name,
            log: log.clone(),
            app: None,
            fail_prepare: false,
            fail_bind: false,
            permanent: false,
            close: CloseMode::Ok,
            on_drain: None,
            _t: PhantomData,
        }
    }

    pub fn fail_prepare(mut self) -> Self {
        self.fail_prepare = true;
        self
    }

    pub fn fail_bind(mut self) -> Self {
        self.fail_bind = true;
        self
    }

    /// A failing `prepare` or `bind` answers an error wrapped in `Permanent`.
    pub fn permanent(mut self) -> Self {
        self.permanent = true;
        self
    }

    pub fn close_with(mut self, mode: CloseMode) -> Self {
        self.close = mode;
        self
    }

    pub fn on_drain(mut self, f: OnDrain) -> Self {
        self.on_drain = Some(f);
        self
    }
}

impl<T> TestServer<T> {
    fn refusal(&self, text: &str) -> BoxError {
        if self.permanent { Box::new(ulo::Permanent::new(Boom::new(text))) } else { Box::new(Boom::new(text)) }
    }
}

impl<T: Transport> ulo::Server for TestServer<T> {
    type Transport = T;

    async fn prepare(&mut self, mounted: ulo::Mounted<'_, T>) -> Result<(), BoxError> {
        self.log.push(format!("prepare {}", self.name));
        self.app = Some(mounted.app().clone());
        if self.fail_prepare { Err(self.refusal("prepare refused")) } else { Ok(()) }
    }

    async fn bind(&mut self, _mounted: ulo::Mounted<'_, T>) -> Result<(), BoxError> {
        self.log.push(format!("bind {}", self.name));
        if self.fail_bind { Err(self.refusal("bind refused")) } else { Ok(()) }
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, token: ulo::DrainToken) {
        self.log.push(format!("drain {}", self.name));
        if let (Some(f), Some(app)) = (&self.on_drain, &self.app) {
            f(app.clone(), token).await;
        }
    }

    async fn close(&self) -> Result<(), BoxError> {
        self.log.push(format!("close {}", self.name));
        match self.close {
            CloseMode::Ok => Ok(()),
            CloseMode::Err => Err(Box::new(Boom::new("close refused"))),
            CloseMode::Panic => panic!("close panics"),
            CloseMode::Pending => std::future::pending().await,
        }
    }
}

/// A server waiting to be queued on a connected app: what [`server`] returns.
pub type Queued = Box<dyn FnOnce(ulo::App<ulo::Connected>) -> ulo::App<ulo::Connected> + Send>;

/// Connects `root` on `builder`, queues `servers` and `listen()`s: the bound app, or the startup
/// error from any step.
pub async fn listened(builder: AppBuilder, servers: Vec<Queued>) -> Result<ulo::App<ulo::app::Bound>, StartupError> {
    watchdog();
    let mut app = builder.wire()?.connect().await?;
    for queue in servers {
        app = queue(app);
    }
    app.listen().await
}

/// A queued server for [`listened`]: `server(TestServer::<TA>::new("a", &log))`.
pub fn server<S: ulo::Server>(s: S) -> Queued {
    Box::new(move |app| app.bind(s))
}

/// The real-clock bound every awaited step runs under, so a hang is a failure.
pub const PATIENCE: Duration = Duration::from_secs(10);

/// `fut` under [`PATIENCE`] on the real clock; panics with `what` when it does not finish.
pub async fn within<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(PATIENCE, fut).await {
        Ok(out) => out,
        Err(_) => panic!("{what} did not finish within {PATIENCE:?}"),
    }
}
