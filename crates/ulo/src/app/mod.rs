//! The application: the graph plus the singleton store, moving through typestates
//! `Wired` → `Connected` → `Bound` → serving → closed (§2, §9.4).
//!
//! From `Connected` on it hands out an [`AppHandle`], a `Clone + Send + Sync` view of the shared
//! state that outlives `serve`. The application value is `Send`.
//!
//! [`Bound`] here is the typestate after `listen()`; the timeout enum is [`crate::Bound`].

pub(crate) mod handle;
pub(crate) mod load;
pub(crate) mod shared;

use std::future::{Future, poll_fn};
use std::marker::PhantomData;
use std::ops::Deref;
use std::pin::{Pin, pin};
use std::sync::{Arc, PoisonError};
use std::task::Poll;
use std::time::Duration;

use crate::app::shared::AppShared;
use crate::binding::Qualifier;
use crate::dependency::Dep;
use crate::dependency::FromContainer;
use crate::error::{
    Closed, ConfigureErrors, ConnectError, FailureReason, LookupError, RuntimeMissing, Shutdown, ShutdownError,
    StartupError,
};
use crate::execution::trace::{TraceMode, TraceSource};
use crate::execution::{ExecOptions, Execution};
use crate::key::{BindingKind, Key};
use crate::resolver::{Purpose, Resolver};
use crate::graph::{ModuleId, wire};
use crate::lifecycle::phase::Phase as Stage;
use crate::lifecycle::{connect, shutdown};
use crate::module::Module;
use crate::module::handle::ModuleRef;
use crate::redact::{Redacted, Redactor, redact};
use crate::runtime::{Clocked, Runtime};
use crate::signal::Signal;
use crate::testing::TestPlan;
use crate::timer::{BoxError, BoxFuture, Defaults, Timer};
use crate::transport::server::{BoundAddr, ErasedServer, Server};

pub use handle::{AppHandle, Phase};

/// Wired: the graph is built and every wiring error has been reported. Nothing is built yet.
pub enum Wired {}

/// Connected: singletons built, readiness checks passed, init and bootstrap hooks run.
pub enum Connected {}

/// Bound: every transport holds its sockets. `serve` consumes this state.
pub enum Bound {}

/// An application in typestate `S`.
///
/// Dropping it lets go of the bound transports' servers, as the end of the shutdown's close step
/// does. For an app that never reaches that step the drop is where they are released: one bound
/// and dropped unserved, one whose `serve` future is dropped, and one whose shutdown was abandoned
/// mid-way. Each server then drops with its last other holder, and that drop is its only shutdown:
/// its `close` does not run, and what only `close` ends, such as a task the server spawned and
/// detached, keeps running.
///
/// A drop while a `close` on a handle is under way, from that close's first poll, leaves the
/// servers to it: its drain and close step run over every server, each server's `close` runs,
/// and the close step's end releases them. A `close` dropped before its close step releases them
/// as it drops, unclosed, as this drop would have; with several under way, the last to end or
/// drop does. A `close` future made before the drop and first polled after it is a `close` after
/// the drop. A `close` polled once and then neither polled again nor dropped holds the servers
/// until it resumes or drops.
///
/// The release awaits nothing and does not panic on a lock: it takes two, each held only to read
/// or change a field and never across an await, and reads either through when poisoned. A
/// server's own drop runs on the thread dropping the app, or on the one ending the last `close`
/// under way.
///
/// An [`AppHandle`] kept past the drop keeps the app's shared state, its singletons among it, and
/// no server once they are released: a `close` started on it after that runs the shutdown's hooks
/// and closes no transport.
pub struct App<S = Wired> {
    pub(crate) shared: Owner,
    /// Queued by `bind` on `Connected`, bound by `listen`.
    pub(crate) servers: Vec<Box<dyn ErasedServer>>,
    _s: PhantomData<S>,
}

/// Configures an app before `wire()`: the root module, the `Timer` or the `Runtime` that carries
/// one, and the four timing knobs. Every knob is timed by the `Timer`; set on a builder with none,
/// each is a wiring error.
pub struct AppBuilder {
    pub(crate) root: Box<dyn Module>,
    pub(crate) config: AppConfig,
    pub(crate) plan: Option<TestPlan>,
}

/// The app's timing configuration. `None` is unset.
#[derive(Clone, Default)]
pub(crate) struct AppConfig {
    /// The clock: the same object as `runtime`, unless `.timer(..)` came after `.runtime(..)`.
    pub(crate) timer: Option<Arc<dyn Timer>>,
    /// What `Dep<dyn Runtime>` resolves to: the app's wrapper around the runtime as given, timing
    /// with its clock or, once a later `.timer(..)` replaced it, with that timer's.
    pub(crate) runtime: Option<Arc<dyn Runtime>>,
    /// The runtime as given to `.runtime(..)`, which a later `.timer(..)` takes the spawning of.
    spawner: Option<Arc<dyn Runtime>>,
    /// The graph's secrets, which the app's runtime redacts a spawned task's panic with. The app
    /// sets them once a graph exists.
    pub(crate) redactor: Redactor,
    pub(crate) drain_timeout: Option<Duration>,
    pub(crate) shutdown_timeout: Option<Duration>,
    pub(crate) hook_timeout: Option<Duration>,
    pub(crate) construct_timeout: Option<Duration>,
    pub(crate) trace: TraceMode,
}

impl AppConfig {
    pub(crate) const DEFAULT_DRAIN: Duration = Duration::from_secs(10);
    pub(crate) const DEFAULT_HOOK: Duration = Duration::from_secs(10);
    pub(crate) const DEFAULT_CONSTRUCT: Duration = Duration::from_secs(30);

    /// The defaults `Bound::Default` resolves against; `None` without a `Timer`, where an item
    /// left at `Default` runs unbounded.
    pub(crate) fn defaults(&self) -> Option<Defaults> {
        if self.timer.is_none() {
            return None;
        }
        Some(Defaults {
            hook_timeout: self.hook_timeout.unwrap_or(Self::DEFAULT_HOOK),
            construct_timeout: self.construct_timeout.unwrap_or(Self::DEFAULT_CONSTRUCT),
        })
    }

    /// The drain window: zero without a `Timer`, which abandons live executions at once.
    pub(crate) fn drain(&self) -> Duration {
        match self.timer {
            Some(_) => self.drain_timeout.unwrap_or(Self::DEFAULT_DRAIN),
            None => Duration::ZERO,
        }
    }

    /// The names of the knobs that were set, for the environment check.
    pub(crate) fn knobs_set(&self) -> Vec<&'static str> {
        [
            ("drain_timeout", self.drain_timeout),
            ("shutdown_timeout", self.shutdown_timeout),
            ("hook_timeout", self.hook_timeout),
            ("construct_timeout", self.construct_timeout),
        ]
        .into_iter()
        .filter_map(|(name, set)| set.map(|_| name))
        .collect()
    }
}

/// The `App` value's hold on the shared state. It moves from typestate to typestate and only
/// `wire` makes one, so an app has one however many `AppHandle`s it hands out, and its drop is the
/// app's. A drop after the close step finds the list of servers empty.
pub(crate) struct Owner(Arc<AppShared>);

impl Deref for Owner {
    type Target = Arc<AppShared>;

    fn deref(&self) -> &Arc<AppShared> {
        &self.0
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        shutdown::app_dropped(&self.0);
    }
}

impl App {
    pub fn builder(root: impl Module) -> AppBuilder {
        AppBuilder { root: Box::new(root), config: AppConfig::default(), plan: None }
    }
}

impl AppBuilder {
    /// The app's one clock; bound for services as `Dep<dyn Timer>` in the core's global module.
    ///
    /// It sets the clock and nothing else, so after [`runtime`](Self::runtime) it replaces only
    /// the runtime's clock: `.runtime(r).timer(t)` spawns on `r` and times with `t`, and
    /// `Dep<dyn Runtime>` then sleeps and reads the time on `t`, so the app has one clock whatever
    /// reads it.
    pub fn timer(mut self, timer: impl Timer) -> Self {
        let timer: Arc<dyn Timer> = Arc::new(timer);
        self.config.runtime = self.config.spawner.as_ref().map(|spawner| {
            Arc::new(Clocked::new(Arc::clone(spawner), Arc::clone(&timer), self.config.redactor.clone())) as Arc<dyn Runtime>
        });
        self.config.timer = Some(timer);
        self
    }

    /// The app's clock and executor; bound for services as `Dep<dyn Runtime>` in the core's global
    /// module, and as `Dep<dyn Timer>`, which resolves to the same object. Everything a `Timer`
    /// alone enables it enables as the clock.
    ///
    /// The object both resolve to is the app's wrapper around `runtime`: it spawns on `runtime`
    /// and times with its clock, and a task spawned through it that panics ends `Panicked` with
    /// every secret the app registered replaced in its message. The app's runtime is the core's
    /// wrapper around the one given, not that object, so code holding `Dep<dyn Runtime>` cannot
    /// reach `runtime`'s own type through it, a downcast included.
    ///
    /// It sets both the spawning and the clock, so it replaces a [`timer`](Self::timer) set
    /// before it: `.timer(t).runtime(r)` spawns and times on `r`.
    pub fn runtime(mut self, runtime: impl Runtime) -> Self {
        let runtime: Arc<dyn Runtime> = Arc::new(runtime);
        let clock = Arc::clone(&runtime) as Arc<dyn Timer>;
        let wrapped: Arc<dyn Runtime> = Arc::new(Clocked::new(Arc::clone(&runtime), clock, self.config.redactor.clone()));
        self.config.timer = Some(Arc::clone(&wrapped) as Arc<dyn Timer>);
        self.config.spawner = Some(runtime);
        self.config.runtime = Some(wrapped);
        self
    }

    /// In-flight executions at shutdown; ten seconds unset.
    pub fn drain_timeout(mut self, d: Duration) -> Self {
        self.config.drain_timeout = Some(d);
        self
    }

    /// The whole close sequence, from the trigger; unset by default. Set it a few seconds under
    /// the orchestrator's grace period.
    pub fn shutdown_timeout(mut self, d: Duration) -> Self {
        self.config.shutdown_timeout = Some(d);
        self
    }

    /// The `Default` every hook starts with; ten seconds unset.
    pub fn hook_timeout(mut self, d: Duration) -> Self {
        self.config.hook_timeout = Some(d);
        self
    }

    /// The `Default` every construction and one attempt of every readiness check starts with;
    /// thirty seconds unset.
    pub fn construct_timeout(mut self, d: Duration) -> Self {
        self.config.construct_timeout = Some(d);
        self
    }

    /// Whether the app reads and writes W3C trace headers; `TraceMode::On` unset (integrations
    /// §6.1). Under `Off` no transport's context reaches an execution, no `TraceSource` is resolved
    /// or called, and `traceparent` and `tracestate` are ordinary headers to every client.
    pub fn trace(mut self, mode: TraceMode) -> Self {
        self.config.trace = mode;
        self
    }

    /// Builds the graph and validates everything, collecting every error in one pass. No
    /// instances, no I/O.
    pub fn wire(self) -> Result<App<Wired>, StartupError> {
        let env = wire::WireEnv {
            timer: self.config.timer.clone(),
            runtime: self.config.runtime.clone(),
            knobs_set: self.config.knobs_set(),
        };
        let graph = wire::wire(self.root, &env, self.plan)?;
        Ok(App::from_shared(Owner(AppShared::new(graph, self.config)), Vec::new()))
    }
}

impl<S> App<S> {
    pub(crate) fn from_shared(shared: Owner, servers: Vec<Box<dyn ErasedServer>>) -> Self {
        App { shared, servers, _s: PhantomData }
    }
}

impl App<Wired> {
    /// Builds the singletons in dependency order, running each one's readiness check right after
    /// it, then every `OnModuleInit` hook, then every `OnApplicationBootstrap` hook (§9.2).
    ///
    /// Then, under `TraceMode::On`, resolves the `dyn TraceSource` the root module sees, if one is
    /// bound, for `AppHandle::trace_source()` to answer (I15). A source that cannot be read, one
    /// two exports answer or one bound per execution, fails the connect as
    /// `ConnectError::Construct` naming it.
    pub async fn connect(self) -> Result<App<Connected>, StartupError> {
        let shared = self.shared;
        shared.phase.advance(Stage::Connecting);
        let graph = shared.graph();
        // `ModuleId`s are positions in collection order, the order the hooks take.
        let modules: Vec<ModuleId> = graph.modules.iter().map(|module| module.id).collect();
        connect::connect(&shared, &graph.connect_order, &modules).await?;
        if shared.config.trace == TraceMode::On {
            let resolver = Resolver::new(&shared, graph.root, None, Purpose::Lookup);
            match <Option<Dep<dyn TraceSource>> as FromContainer>::from_container(&resolver).await {
                Ok(Some(source)) => {
                    let _ = shared.trace_source.set(source.into_arc());
                }
                Ok(None) => {}
                Err(error) => return Err(StartupError::Connect(trace_source_failure(&graph, error))),
            }
        }
        shared.phase.advance(Stage::Running);
        Ok(App::from_shared(shared, Vec::new()))
    }
}

/// A `dyn TraceSource` lookup at `connect` that answered neither a source nor `NotFound`.
fn trace_source_failure(graph: &crate::graph::Graph, error: LookupError) -> ConnectError {
    let key = Key::of::<dyn TraceSource, ()>().name(BindingKind::Single);
    let module = graph.module(graph.root).name.clone();
    let reason = match error {
        LookupError::Construct { reason, .. } => reason,
        other => {
            let text = other.to_string();
            FailureReason::Errored(Redacted::from_parts(Box::new(other), text))
        }
    };
    ConnectError::Construct { key, module, reason }
}

impl App<Connected> {
    /// Queues a transport; `listen` binds every queued transport.
    pub fn bind(mut self, server: impl Server) -> App<Connected> {
        self.servers.push(Box::new(server));
        self
    }

    /// Prepares every queued transport, then binds them in the order they were queued
    /// (transports DESIGN §2.7, X6).
    ///
    /// 1. `prepare` on every server: route tables, duplicate checks, TLS loading, CORS
    ///    validation, endpoint parsing, inherited sockets. Every failure is collected and
    ///    reported together as `StartupError::Configure`; nothing has bound yet, so a
    ///    configuration error is always reported before a port conflict.
    /// 2. `bind` on each server in order. When one fails, those already bound are closed before
    ///    the error returns, so a refused app holds no sockets.
    ///
    /// Refuses an app that binds a transport with no `Runtime` before either step, one given
    /// `.timer(..)` alone included: `StartupError::Bind` carrying `RuntimeMissing` in its `source`.
    pub async fn listen(mut self) -> Result<App<Bound>, StartupError> {
        let mut queued = std::mem::take(&mut self.servers);
        if self.shared.config.runtime.is_none()
            && let Some(server) = queued.first()
        {
            let transport = server.transport_name();
            let refusal = RuntimeMissing { transport };
            let text = refusal.to_string();
            return Err(StartupError::Bind { transport, source: Redacted::from_parts(Box::new(refusal), text) });
        }
        let handle = self.handle();

        let mut refused = Vec::new();
        for server in &mut queued {
            if let Err(error) = server.prepare(&handle).await {
                refused.push((server.transport_name(), error));
            }
        }
        if !refused.is_empty() {
            return Err(StartupError::Configure(ConfigureErrors::collect(refused, &self.shared.graph().secrets)));
        }

        let mut bound: Vec<Arc<dyn ErasedServer>> = Vec::with_capacity(queued.len());
        for mut server in queued {
            let transport = server.transport_name();
            if let Err(error) = server.bind(&handle).await {
                let source = redact(&self.shared.graph().secrets, error);
                for done in &bound {
                    let _ = done.close().await;
                }
                return Err(StartupError::Bind { transport, source });
            }
            bound.push(Arc::from(server));
        }
        *self.shared.servers.lock().unwrap_or_else(PoisonError::into_inner) = bound;
        Ok(App::from_shared(self.shared, Vec::new()))
    }

    pub fn handle(&self) -> AppHandle {
        AppHandle { shared: Arc::clone(&self.shared) }
    }

    /// `T` with the root module's visibility.
    pub async fn get<T: ?Sized + Send + Sync + 'static>(&self) -> Result<Dep<T>, LookupError> {
        self.shared.get_root::<T>().await
    }

    /// The one module of type `M`: `LookupError::AmbiguousModule` if several configurations of
    /// it are registered. `M` is the type a module's identity names, the owner type for a
    /// `DynamicModule`.
    pub fn module<M: 'static>(&self) -> Result<ModuleRef, LookupError> {
        self.shared.find_module::<M>(None)
    }

    /// The keyed instance `Q` of `M`.
    pub fn module_keyed<M: 'static, Q: 'static>(&self) -> Result<ModuleRef, LookupError> {
        self.shared.find_module::<M>(Some(Qualifier::of::<Q>()))
    }

    /// A standalone execution with the root module's visibility (§6.3). Refused from Draining on
    /// with `Closed`. `execute` drops the execution when the closure's future completes, which
    /// fires no cancellation; a subtask that outlives it takes `exec.handle()`.
    ///
    /// An inherent `async fn`, so its future is `Send` whenever the closure's future is.
    pub async fn execute<F, R>(&self, opts: ExecOptions, f: F) -> Result<R, Closed>
    where
        F: AsyncFnOnce(&Execution) -> R,
    {
        let root = self.shared.graph().root;
        self.shared.execute(root, opts, f).await
    }

    /// Runs the shutdown sequence with no sockets to close: for a job, a CLI command or a test
    /// that stops after `connect()`.
    pub async fn close(self, signal: Signal) -> Result<Shutdown, ShutdownError> {
        shutdown::close(&self.shared, signal).await
    }
}

impl App<Bound> {
    pub fn handle(&self) -> AppHandle {
        AppHandle { shared: Arc::clone(&self.shared) }
    }

    /// Every address the bound transports listen on, in bind order: port 0 reports the port the
    /// OS chose (transports DESIGN §2.7, X6). Empty once the shutdown's close step has run, whatever
    /// each server's `close` returned: a close that timed out, errored or panicked releases its
    /// server too.
    pub fn addresses(&self) -> Vec<BoundAddr> {
        let servers = self.shared.servers.lock().unwrap_or_else(PoisonError::into_inner);
        servers.iter().flat_map(|server| server.bound()).collect()
    }

    /// Serves until `signal` resolves or a `close` on any handle, whichever comes first, then
    /// runs the shutdown sequence and returns its outcome (§9.5).
    ///
    /// Dropping the returned future releases the servers without running their `close`; see
    /// [`App`].
    ///
    /// Each transport's `Server::serve` is polled from here until the shutdown's outcome, so a
    /// transport keeps serving through the before-shutdown stage and the drain. One that fails
    /// before any trigger starts the shutdown itself, under a signal naming the transport and its
    /// redacted error; one that returns `Ok` early is simply done.
    pub async fn serve(self, signal: impl Future<Output = Signal> + Send) -> Result<Shutdown, ShutdownError> {
        let shared = self.shared;
        let servers: Vec<Arc<dyn ErasedServer>> = shared.servers.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let mut serving: Vec<Option<BoxFuture<'_, Result<(), BoxError>>>> =
            servers.iter().map(|server| Some(server.serve())).collect();

        let trigger = {
            let mut signal = pin!(signal);
            let mut triggered = shared.shutdown.triggered();
            poll_fn(|cx| {
                if let Poll::Ready(own) = signal.as_mut().poll(cx) {
                    return Poll::Ready(own);
                }
                // A `close` won; its sequence is already running, and `close` below joins it
                // whatever signal it is handed.
                if Pin::new(&mut triggered).poll(cx).is_ready() {
                    return Poll::Ready(shared.shutdown.winning_signal().unwrap_or_else(|| Signal::new("close")));
                }
                for (server, slot) in servers.iter().zip(serving.iter_mut()) {
                    let Some(fut) = slot else { continue };
                    let Poll::Ready(result) = fut.as_mut().poll(cx) else { continue };
                    *slot = None;
                    if let Err(error) = result {
                        let reason = redact(&shared.graph().secrets, error);
                        return Poll::Ready(Signal::new(format!("transport `{}` failed: {reason}", server.transport_name())));
                    }
                }
                Poll::Pending
            })
            .await
        };

        let mut closing = pin!(shutdown::close(&shared, trigger));
        poll_fn(|cx| {
            for slot in serving.iter_mut() {
                if let Some(fut) = slot
                    && fut.as_mut().poll(cx).is_ready()
                {
                    *slot = None;
                }
            }
            closing.as_mut().poll(cx)
        })
        .await
    }
}
