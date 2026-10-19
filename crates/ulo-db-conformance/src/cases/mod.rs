//! The scenarios of integrations DESIGN §4.9, one function per scenario, each generic over the
//! [`Host`] and each naming the planted break it is shown failing against.
//!
//! Every scenario builds its apps here: a root module importing what the scenario names, on the
//! harness's runtime. No app binds a transport; an app stops at `Connected`, which reads as
//! `Running` to the health module, and closes through `App::close`, whose shutdown runs the same
//! sequence a served app's does, the drain included.

pub mod close;
pub mod connect;
pub mod health;
pub mod keyed;
pub mod lazy;
pub mod migrate;
pub mod runtime;
pub mod stall;
pub mod wire;

use std::error::Error;
use std::future::Future;
use std::pin::pin;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use futures_util::future::{Either, select};
use ulo::{
    App, AppBuilder, ConnectError, Connected, ConstructError, FailureReason, Module, ModuleDef, ModuleIdentity,
    Permanent, Redacted, Signal, StartupError, TaskEnd, Timer, ValueHandle, Wired,
};
use ulo_db::{Store, StoreModule};
use ulo_health::{Report, Readiness};

use crate::{Harness, Host, PASSWORD, report};

/// The client the host's store binds.
pub(crate) type Client<H> = <<H as Host>::Store as Store>::Client;

/// Retries for a scenario that wants the connect bound, and not the retry count, to end the loop.
pub(crate) const RETRIES: u32 = 1_000;

/// The constant wait between attempts in every scenario that retries.
pub(crate) const BACKOFF: Duration = Duration::from_millis(100);

/// A connect bound no scenario expects to reach: a failure well inside it was not retried to it.
pub(crate) const PATIENT: Duration = Duration::from_secs(20);

/// One module the root imports, taken on the first `register`, which `wire` calls once.
trait Import: Send {
    fn import(self: Box<Self>, m: &mut ModuleDef<'_>);
}

struct Imported<M>(M);

impl<M: Module> Import for Imported<M> {
    fn import(self: Box<Self>, m: &mut ModuleDef<'_>) {
        m.import(self.0);
    }
}

/// The root module of every app a scenario builds: it imports what the scenario names.
pub(crate) struct Root {
    imports: Mutex<Vec<Box<dyn Import>>>,
}

impl Root {
    pub(crate) fn new() -> Root {
        Root { imports: Mutex::new(Vec::new()) }
    }

    pub(crate) fn import<M: Module>(self, module: M) -> Root {
        lock(&self.imports).push(Box::new(Imported(module)));
        self
    }
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let imports = std::mem::take(&mut *lock(&self.imports));
        for import in imports {
            import.import(m);
        }
    }
}

/// A store module over `options`, with the module's defaults.
pub(crate) fn store<H: Host>(options: <H::Store as Store>::Options) -> StoreModule<H::Store> {
    StoreModule::from_options(options)
}

/// A store module over `options` whose one retry loop runs until `within`, so the bound and not the
/// retry count ends it.
pub(crate) fn bounded<H: Host>(options: <H::Store as Store>::Options, within: Duration) -> StoreModule<H::Store> {
    store::<H>(options).connect_within(within).retries(RETRIES).backoff(BACKOFF)
}

/// An app over `root` on the harness's runtime.
pub(crate) fn builder<H: Host>(root: Root) -> AppBuilder {
    App::builder(root).runtime(<H::Harness as Harness>::runtime())
}

/// `builder`'s app wired; a wiring refusal fails the scenario, since every scenario but the
/// malformed-URL one wires options the store accepts.
pub(crate) fn wired(builder: AppBuilder, what: &str) -> App<Wired> {
    builder.wire().unwrap_or_else(|error| crate::startup_failed!("{what} did not wire: {}", report(&error)))
}

/// `builder`'s app wired and connected, with how long `connect` took.
pub(crate) async fn connecting(builder: AppBuilder, what: &str) -> (Result<App<Connected>, StartupError>, Duration) {
    let app = wired(builder, what);
    let began = Instant::now();
    let outcome = app.connect().await;
    (outcome, began.elapsed())
}

/// `builder`'s app connected; a refusal fails the scenario.
pub(crate) async fn connected(builder: AppBuilder, what: &str) -> App<Connected> {
    match connecting(builder, what).await.0 {
        Ok(app) => app,
        Err(error) => crate::startup_failed!("{what} did not connect: {}", report(&error)),
    }
}

/// `builder`'s app refused at `connect`, with how long the refusal took. An app that connects is
/// closed and fails the scenario with `unexpected`.
pub(crate) async fn refused(builder: AppBuilder, what: &str, unexpected: &str) -> (StartupError, Duration) {
    match connecting(builder, what).await {
        (Err(error), took) => (error, took),
        (Ok(app), took) => {
            let _ = app.close(Signal::new("conformance")).await;
            panic!("{what} connected after {took:?}: {unexpected}");
        }
    }
}

/// Closes `app`, failing the scenario when the shutdown reports a failure: a destroy hook, the
/// store's close among them, that timed out or errored.
pub(crate) async fn closed(app: App<Connected>, what: &str) -> ulo::Shutdown {
    app.close(Signal::new("conformance")).await.unwrap_or_else(|error| panic!("{what} did not close cleanly: {error}"))
}

/// The health module's `Readiness`, which the root sees through its import.
pub(crate) async fn readiness(app: &App<Connected>) -> Readiness {
    match app.get::<Readiness>().await {
        Ok(readiness) => (*readiness).clone(),
        Err(error) => crate::startup_failed!("the app imports the health module and has no `Readiness`: {error}"),
    }
}

/// Whether every check the store contributed reads `up`, `None` when the report holds none. A
/// keyed store contributes one check per instance, each named from `Store::NAME`.
pub(crate) fn store_reads<S: Store>(report: &Report, up: bool) -> Option<bool> {
    let mut checks = report.results().iter().filter(|result| result.name().contains(S::NAME)).peekable();
    checks.peek()?;
    Some(checks.all(|result| result.health().is_up() == up))
}

/// How many checks the store contributed to `report`.
pub(crate) fn store_checks<S: Store>(report: &Report) -> usize {
    report.results().iter().filter(|result| result.name().contains(S::NAME)).count()
}

/// The names of the checks the store contributed to `report`.
pub(crate) fn store_check_names<S: Store>(report: &Report) -> Vec<String> {
    report.results().iter().map(|result| result.name()).filter(|name| name.contains(S::NAME)).map(str::to_owned).collect()
}

/// Every check of `report`, up or down with its reason, for a failure message.
pub(crate) fn describe(report: &Report) -> String {
    if report.results().is_empty() {
        return "no checks".to_owned();
    }
    report
        .results()
        .iter()
        .map(|result| match result.health() {
            ulo_health::Health::Up => format!("`{}` up", result.name()),
            ulo_health::Health::Down(reason) => format!("`{}` down: {reason}", result.name()),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Polls readiness until it reads `up` and every check the store contributed agrees, or until
/// `within` passes. Answers whether it held, with the last report.
pub(crate) async fn readiness_reads<H: Host>(readiness: &Readiness, up: bool, within: Duration) -> (bool, Arc<Report>) {
    let deadline = Instant::now() + within;
    loop {
        let report = readiness.readiness().await;
        if report.is_up() == up && store_reads::<H::Store>(&report, up) == Some(true) {
            return (true, report);
        }
        if Instant::now() >= deadline {
            return (false, report);
        }
        sleep::<H>(Duration::from_millis(100)).await;
    }
}

/// The connect failure under `error` and its reason; `None` for a failure of another phase.
pub(crate) fn connect_failure(error: &StartupError) -> Option<(&ConnectError, &FailureReason)> {
    let StartupError::Connect(connect) = error else { return None };
    let reason = match connect {
        ConnectError::Construct { reason, .. } | ConnectError::Readiness { reason, .. } | ConnectError::Hook { reason, .. } => reason,
        _ => return None,
    };
    Some((connect, reason))
}

/// Bounds the walks below, in case a `source()` chain loops back on itself.
const DEPTH: usize = 32;

/// An `E` the connect failure under `error` carries, through the `Redacted` the core stores it in,
/// the `Permanent` around it and any `source()` chain.
pub(crate) fn carried<E: Error + 'static>(error: &StartupError) -> Option<&E> {
    match connect_failure(error)? {
        (_, FailureReason::Errored(redacted)) => in_redacted::<E>(redacted, 0),
        _ => None,
    }
}

/// A `Redacted` hands its original out by type alone, so each wrapper the core or an integration
/// puts around an error is tried by name.
fn in_redacted<E: Error + 'static>(redacted: &Redacted, depth: usize) -> Option<&E> {
    if depth >= DEPTH {
        return None;
    }
    if let Some(found) = redacted.downcast_ref::<E>() {
        return Some(found);
    }
    if let Some(permanent) = redacted.downcast_ref::<Permanent>() {
        return in_chain::<E>(permanent, depth + 1);
    }
    match redacted.downcast_ref::<ConstructError>()? {
        ConstructError::Failed(inner) => {
            let inner: &(dyn Error + 'static) = &**inner;
            in_chain::<E>(inner, depth + 1)
        }
        _ => None,
    }
}

fn in_chain<'a, E: Error + 'static>(error: &'a (dyn Error + 'static), depth: usize) -> Option<&'a E> {
    if depth >= DEPTH {
        return None;
    }
    if let Some(found) = error.downcast_ref::<E>() {
        return Some(found);
    }
    if let Some(redacted) = error.downcast_ref::<Redacted>() {
        return in_redacted::<E>(redacted, depth + 1);
    }
    error.source().and_then(|cause| in_chain::<E>(cause, depth + 1))
}

/// Fails the scenario when any rendering of `error` prints [`PASSWORD`]: its `Display`, its
/// `Debug` and its chain as [`report`] joins it.
pub(crate) fn assert_redacted(error: &StartupError, what: &str) {
    for text in [error.to_string(), format!("{error:?}"), report(error)] {
        assert!(!text.contains(PASSWORD), "{what} prints the password: {text}");
    }
}

pub(crate) async fn sleep<H: Host>(d: Duration) {
    <H::Harness as Harness>::runtime().sleep(d).await;
}

/// `fut` as a task of the scenario's runtime, answering its value.
pub(crate) fn spawn<H, T, F>(fut: F) -> ValueHandle<T>
where
    H: Host,
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    ulo::spawn_with(&<H::Harness as Harness>::runtime(), fut)
}

/// `fut`'s output, or `None` once `within` passes on the scenario's clock.
pub(crate) async fn timeout<H: Host, T>(within: Duration, fut: impl Future<Output = T>) -> Option<T> {
    let expired = <H::Harness as Harness>::runtime().sleep(within);
    match select(pin!(fut), expired).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(((), _)) => None,
    }
}

/// Polls `done` until it holds or `within` passes; answers whether it held.
pub(crate) async fn eventually<H: Host>(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep::<H>(Duration::from_millis(20)).await;
    }
}

/// A spawned task's value, or a failure naming how the task ended instead.
pub(crate) fn joined<T>(ended: Result<T, TaskEnd>, what: &str) -> T {
    ended.unwrap_or_else(|end| panic!("{what} did not return a value: it ended {end:?}"))
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
