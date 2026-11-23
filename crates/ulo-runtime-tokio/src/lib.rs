//! The tokio runtime for `ulo` (transports DESIGN §8): [`run`], which serves an application on a
//! tokio runtime it builds until the OS asks the process to stop; the core's
//! [`Runtime`](ulo::Runtime) as [`Tokio`], its clock alone as [`Timer`], the OS signal future
//! [`shutdown_signal`] for `serve`, and [`spawn`] and [`spawn_in`]. A tokio-based RPC link holds a
//! `Tokio` of its own and runs its I/O there through [`Tokio::run`].
//!
//! ```no_run
//! # #[ulo::module]
//! # struct AppModule;
//! # fn main_with(server: impl ulo::Server) -> Result<(), Box<dyn std::error::Error>> {
//! // `server` is a transport's, `ulo_http_hyper::Server::new("127.0.0.1:3000")` for one.
//! ulo_runtime_tokio::run(AppModule, server)?;
//! # Ok(())
//! # }
//! ```
//!
//! An application that sets a builder knob, reads the app between two phases or ends on a future
//! of its own writes the phases `run` performs, on a runtime it starts itself:
//!
//! ```
//! # use ulo::App;
//! # #[ulo::module]
//! # struct AppModule;
//! # async fn f() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let app = App::builder(AppModule).runtime(ulo_runtime_tokio::Tokio::current()).wire()?;
//! // ..
//! # let app = app.connect().await?.listen().await?;
//! app.serve(ulo_runtime_tokio::shutdown_signal()).await?;
//! # Ok(())
//! # }
//! ```

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use ulo::{
    App, BoxFuture, Connected, ExecutionRef, Module, Refused, RuntimeShutDown, RuntimeTask, Server, Shutdown, ShutdownError, Signal,
    StartupError, TaskHandle,
};

/// The app's runtime on one tokio runtime, held by its `Handle`: tasks are spawned on that
/// runtime, and sleeps registered with its timer, from whichever thread asks, a worker of another
/// runtime or a plain thread with none, as a client's drop may run on. Set with
/// `AppBuilder::runtime(ulo_runtime_tokio::Tokio::current())`, or given to a client built outside an app.
/// A tokio-based RPC link holds one of its own, whatever runtime the app was given.
///
/// A runtime that has shut down refuses, as a value and never a panic: `try_spawn` and
/// `try_sleep` answer `Err(Refused)`, `spawn` drops the task unrun and its handle answers
/// `Aborted`, a sleep ends at once, the shutdown falling before the sleep or during it, and `now`
/// still answers. [`timer`](Self::timer) is the clock alone, refusing alike.
#[derive(Clone, Debug)]
pub struct Tokio {
    handle: Handle,
    io: Arc<OnceLock<bool>>,
}

/// A tokio runtime built without its IO driver (`enable_io`), asked to register a socket, which
/// panics. Answered by [`Tokio::require_io`] as a value so a link refuses the configuration
/// instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoIoDriver;

const _: () = assert!(size_of::<NoIoDriver>() < 128, "clippy's `result_large_err` fires at 128 bytes");

impl fmt::Display for NoIoDriver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the tokio runtime was built without its IO driver: call `enable_io()` (or `enable_all()`) on its `Builder`")
    }
}

impl Error for NoIoDriver {}

impl Tokio {
    /// The tokio runtime current on the calling thread. Panics outside one, at this call, as
    /// `Handle::current` does; nothing after it looks for a runtime.
    pub fn current() -> Tokio {
        Tokio::from_handle(Handle::current())
    }

    pub fn from_handle(handle: Handle) -> Tokio {
        Tokio { handle, io: Arc::new(OnceLock::new()) }
    }

    /// The tokio runtime current on the calling thread, `None` outside one.
    pub fn try_current() -> Option<Tokio> {
        Handle::try_current().ok().map(Tokio::from_handle)
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// Whether sockets can register on the held runtime, as `Err(NoIoDriver)` when its builder
    /// left out `enable_io`. tokio offers no query for the IO driver, and registering a socket on
    /// a runtime without one panics, so the probe registers one end of a socket pair under
    /// `catch_unwind`. A pair needs no network on Unix; elsewhere the probe is a loopback listener.
    ///
    /// A runtime that refuses leaves the panic hook to print its message once per `Tokio` value
    /// (clones share the answer), and a healthy runtime prints nothing. The answer is cached, so
    /// a later call costs a load. A probe that cannot find out, because the socket cannot be made
    /// or the runtime has shut down, answers `Ok` and caches nothing; whatever the caller does
    /// next reports what it meets.
    pub fn require_io(&self) -> Result<(), NoIoDriver> {
        let enabled = match self.io.get() {
            Some(enabled) => *enabled,
            None => match probe_io(&self.handle) {
                Some(enabled) => *self.io.get_or_init(|| enabled),
                None => true,
            },
        };
        if enabled { Ok(()) } else { Err(NoIoDriver) }
    }

    /// The held runtime's clock alone, for `AppBuilder::timer`.
    pub fn timer(&self) -> Timer {
        Timer { handle: self.handle.clone() }
    }

    /// `fut` run as a task on the held runtime, its output answered to whichever executor awaits
    /// the returned future: a tokio worker, another runtime's, or a plain thread with none. What a
    /// tokio-based RPC link does with tokio's reactor, timer or spawner goes through here, so the
    /// link's futures do not depend on the caller's executor.
    ///
    /// The task is spawned when the answer is first polled, so answers created in order and
    /// awaited in order run in order. Dropping the answer aborts the task. A panic in the task
    /// resumes where the answer is awaited. A runtime that has shut down drops the task unrun,
    /// answered as `Err(RuntimeShutDown)`, its source the runtime's `Refused`.
    pub fn run<F>(&self, fut: F) -> impl Future<Output = Result<F::Output, RuntimeShutDown>> + Send + 'static
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let handle = self.handle.clone();
        async move {
            match Aborting(handle.spawn(fut)).await {
                Ok(output) => Ok(output),
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(_) => Err(RuntimeShutDown::new(Refused)),
            }
        }
    }
}

/// `None` when the probe could not find out.
fn probe_io(handle: &Handle) -> Option<bool> {
    #[cfg(unix)]
    let registered = {
        let (probe, _peer) = std::os::unix::net::UnixStream::pair().ok()?;
        probe.set_nonblocking(true).ok()?;
        let _entered = handle.enter();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tokio::net::UnixStream::from_std(probe).map(drop)))
    };
    #[cfg(not(unix))]
    let registered = {
        let probe = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).ok()?;
        probe.set_nonblocking(true).ok()?;
        let _entered = handle.enter();
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tokio::net::TcpListener::from_std(probe).map(drop)))
    };
    match registered {
        Ok(Ok(())) => Some(true),
        // An `Err` is the runtime's shutdown or a registration failure, neither of which says
        // anything about the driver.
        Ok(Err(_)) => None,
        Err(_) => Some(false),
    }
}

/// A task's `JoinHandle` that aborts the task when dropped.
struct Aborting<T>(JoinHandle<T>);

impl<T> Future for Aborting<T> {
    type Output = Result<T, tokio::task::JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> Drop for Aborting<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl ulo::Timer for Tokio {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        sleep_on(&self.handle, d)
    }

    fn now(&self) -> Instant {
        now_on(&self.handle)
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        try_sleep_on(&self.handle, d)
    }
}

/// `now` enters `handle`, so it reads that runtime's clock, a paused one included, wherever it is
/// called.
fn now_on(handle: &Handle) -> Instant {
    let _entered = handle.enter();
    tokio::time::Instant::now().into_std()
}

fn sleep_on(handle: &Handle, d: Duration) -> BoxFuture<'static, ()> {
    let sleep = try_sleep_on(handle, d);
    Box::pin(async move {
        let _ = sleep.await;
    })
}

/// The sleep runs as a task of `handle`'s runtime. tokio's timer panics when a sleep is polled
/// after the runtime has shut down, wherever it is polled, and a runtime drops its tasks before
/// it shuts its timer down, so a sleep inside one is never polled then: the runtime drops it and
/// the task's `JoinHandle` reports that. The deadline is read here, as `tokio::time::sleep` reads
/// it at its call; the task is spawned at the first poll.
fn try_sleep_on(handle: &Handle, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
    let deadline = {
        let _entered = handle.enter();
        let now = tokio::time::Instant::now();
        // `tokio::time::sleep`'s own answer to a duration past what an `Instant` holds.
        now.checked_add(d).unwrap_or_else(|| now + Duration::from_secs(86400 * 365 * 30))
    };
    let handle = handle.clone();
    Box::pin(async move {
        // The task's future cannot panic, so an error is the runtime dropping it.
        Aborting(handle.spawn(async move { tokio::time::sleep_until(deadline).await })).await.map_err(|_| Refused)
    })
}

impl ulo::Spawn for Tokio {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        TaskHandle::launch(fut, |task| Task(Some(self.handle.spawn(task))))
    }

    /// tokio offers no check for a runtime that has shut down; one drops a task inside `spawn`,
    /// so the task's `JoinHandle` answers on its first poll. The wrapper the core spawns cannot
    /// panic and nothing has aborted the task yet, so an error there is a shutdown dropping it.
    /// Refused: the task was not started, or a shutdown stopped it before it finished, a worker
    /// having polled it between `spawn` and that first poll.
    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        let mut refused = false;
        let task = TaskHandle::launch(fut, |task| {
            let mut join = self.handle.spawn(task);
            // Unconstrained: an exhausted cooperative budget would answer `Pending` for a task
            // already dropped.
            match Pin::new(&mut tokio::task::unconstrained(&mut join)).poll(&mut Context::from_waker(Waker::noop())) {
                Poll::Pending => Task(Some(join)),
                Poll::Ready(Ok(())) => Task(None),
                Poll::Ready(Err(_)) => {
                    refused = true;
                    Task(None)
                }
            }
        });
        if refused { Err(Refused) } else { Ok(task) }
    }

    /// On the held runtime's blocking pool, `Handle::spawn_blocking`, whose threads the runtime
    /// starts as work needs them, up to its builder's `max_blocking_threads`, 512 unset. A runtime
    /// that has shut down drops the work unrun. A runtime shutting down waits for work under way,
    /// unbounded unless its owner shuts it down with `shutdown_timeout`.
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>) -> TaskHandle {
        TaskHandle::launch_blocking(work, |work| Task(Some(self.handle.spawn_blocking(work))))
    }

    fn name(&self) -> &'static str {
        "tokio"
    }
}

/// tokio's handle as the core's `RuntimeTask`. A `JoinHandle` resolves only once its future has
/// been dropped, after an abort as after a return, which is what `poll_ended` requires; dropping
/// it detaches the task. `None` once the handle has answered: polled again, a `JoinHandle` panics.
struct Task(Option<JoinHandle<()>>);

impl RuntimeTask for Task {
    fn abort(&mut self) {
        if let Some(join) = &self.0 {
            join.abort();
        }
    }

    fn poll_ended(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let Some(join) = self.0.as_mut() else { return Poll::Ready(()) };
        let ended = Pin::new(join).poll(cx).map(|_| ());
        if ended.is_ready() {
            self.0 = None;
        }
        ended
    }

    fn detach(self: Box<Self>) {}
}

/// The clock of one tokio runtime, held by its `Handle`, without its spawning: `AppBuilder::timer`
/// for an app that times on tokio and spawns on nothing or on another runtime. It times as
/// [`Tokio`] times, a paused test clock (`tokio::time::pause`) driving deadlines and sleeps
/// together, and refuses as `Tokio` refuses once its runtime has shut down: `try_sleep` answers
/// `Err(Refused)`, a sleep ends at once, and `now` still answers.
///
/// Built from a runtime the caller has, [`Tokio::timer`] or [`Timer::from_handle`], or captured
/// once with [`Timer::current`] or [`Timer::try_current`]; nothing after that looks for a runtime.
#[derive(Clone, Debug)]
pub struct Timer {
    handle: Handle,
}

impl Timer {
    /// The clock of the tokio runtime current on the calling thread. Panics outside one, at this
    /// call, as `Handle::current` does.
    pub fn current() -> Timer {
        Timer { handle: Handle::current() }
    }

    /// The clock of the tokio runtime current on the calling thread, `None` outside one.
    pub fn try_current() -> Option<Timer> {
        Handle::try_current().ok().map(|handle| Timer { handle })
    }

    pub fn from_handle(handle: Handle) -> Timer {
        Timer { handle }
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }
}

impl ulo::Timer for Timer {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        sleep_on(&self.handle, d)
    }

    fn now(&self) -> Instant {
        now_on(&self.handle)
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        try_sleep_on(&self.handle, d)
    }
}

/// Resolves on the first shutdown signal the OS delivers: SIGINT or SIGTERM on Unix, Ctrl-C or
/// Ctrl-Close on Windows, as a `Signal` named after it (`"SIGTERM"`, `"SIGINT"`, `"CTRL_C"`,
/// `"CTRL_CLOSE"`). A handler that cannot be installed is skipped and the others still resolve it;
/// with none installed, or on another platform, the future never resolves.
///
/// The handlers are installed when the future is first polled, inside the tokio runtime that
/// polls it, so the function may be called anywhere. On Unix tokio keeps a handler installed for
/// the rest of the process, so a second SIGINT during the shutdown sequence does not end it.
pub fn shutdown_signal() -> impl Future<Output = Signal> + Send + 'static {
    os::shutdown_signal()
}

#[cfg(unix)]
mod os {
    use tokio::signal::unix::{SignalKind, signal};
    use ulo::Signal;

    pub(super) async fn shutdown_signal() -> Signal {
        let sigterm = signal(SignalKind::terminate()).ok();
        let sigint = signal(SignalKind::interrupt()).ok();
        let terminate = async move {
            if let Some(mut stream) = sigterm
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        let interrupt = async move {
            if let Some(mut stream) = sigint
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = terminate => Signal::new("SIGTERM"),
            () = interrupt => Signal::new("SIGINT"),
        }
    }
}

#[cfg(windows)]
mod os {
    use tokio::signal::windows::{ctrl_c, ctrl_close};
    use ulo::Signal;

    pub(super) async fn shutdown_signal() -> Signal {
        let on_ctrl_c = ctrl_c().ok();
        let on_ctrl_close = ctrl_close().ok();
        let ctrl_c = async move {
            if let Some(mut stream) = on_ctrl_c
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        let ctrl_close = async move {
            if let Some(mut stream) = on_ctrl_close
                && stream.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            () = ctrl_c => Signal::new("CTRL_C"),
            () = ctrl_close => Signal::new("CTRL_CLOSE"),
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use ulo::Signal;

    pub(super) async fn shutdown_signal() -> Signal {
        std::future::pending().await
    }
}

/// Spawns `fut` on the current tokio runtime.
pub fn spawn<F>(fut: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    tokio::spawn(fut)
}

/// Spawns `fut` holding `exec` for the task's lifetime, so the execution's cache and its
/// execution-scoped instances stay alive while the task runs, and stops the task when the
/// execution is cancelled: the handle answers `None` then, `Some` with the output otherwise.
///
/// The drain tracks executions, not tasks; a task that should hold shutdown open runs inside one
/// this way rather than detached.
pub fn spawn_in<F>(exec: &ExecutionRef, fut: F) -> JoinHandle<Option<F::Output>>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    let exec = exec.clone();
    tokio::spawn(async move {
        tokio::select! {
            biased;
            () = exec.cancelled() => None,
            out = fut => Some(out),
        }
    })
}

/// Serves `root` on a tokio runtime of its own until the OS asks the process to stop, for a
/// `main` with no `#[tokio::main]`. It is the startup chain with no knob set, the runtime built as
/// `#[tokio::main]` builds one, multi-threaded with its I/O and time drivers:
///
/// ```ignore
/// # // reason: the chain `run` performs, written out; `bind_to` queues each server `servers` holds
/// runtime.block_on(async {
///     // Polled once here, which installs the handlers.
///     let signal = shutdown_signal();
///     let app = App::builder(root).runtime(Tokio::current()).wire()?.connect().await?;
///     let app = servers.bind_to(app).listen().await?;
///     app.serve(signal).await
/// })
/// ```
///
/// `servers` is one server or a tuple of them, bound in the order written. Each address the app
/// listens on is logged at `info` through `tracing`, as `ulo.transport` and `address`, once
/// `listen` has bound them all. [`shutdown_signal`] ends it: SIGINT or SIGTERM on Unix, Ctrl-C or
/// Ctrl-Close on Windows. Its handlers are installed before `wire`, so a signal that arrives while
/// `connect` runs hooks and readiness checks is held: the app finishes starting and `serve` shuts
/// it down, where the signal's default action would end the process with no hook run. The app
/// then runs its shutdown sequence, its hooks, the drain and each server's `close`, and `run`
/// answers that shutdown's [`Shutdown`], or [`RunError::Shutdown`] listing its failures. A
/// startup that fails answers [`RunError::Startup`] with nothing served, whatever had initialized
/// torn down as `connect` tears it down. The runtime is shut down before `run` returns without
/// waiting for a blocking task still running, which a drop, and so the end of a `#[tokio::main]`,
/// waits for with no bound.
///
/// An application that sets a builder knob (`drain_timeout`, `shutdown_timeout`, `unserved`, a
/// test clock), reads the app between two phases, ends on a future of its own or runs on a
/// runtime it configures writes the chain itself, on a runtime it starts.
///
/// # Panics
///
/// Inside a tokio runtime, as `Runtime::block_on` does: `run` starts the process's runtime, and
/// an `async fn main` already has one.
#[expect(clippy::result_large_err, reason = "`StartupError` is the core's, and `RunError` holds it unchanged")]
pub fn run<M>(root: impl Module, servers: impl Servers<M>) -> Result<Shutdown, RunError> {
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(RunError::Runtime)?;
    let tokio = Tokio::from_handle(runtime.handle().clone());
    let answer = runtime.block_on(async move {
        let signal = installed(shutdown_signal());
        let app = App::builder(root).runtime(tokio).wire()?.connect().await?;
        let app = servers.bind_to(app).listen().await?;
        for bound in app.addresses() {
            tracing::info!(ulo.transport = bound.transport, address = %bound.addr, "listening");
        }
        Ok(app.serve(signal).await?)
    });
    // The app has drained, closed and abandoned what outlived the drain; a drop would also wait for
    // each blocking task still running, which no deadline bounds.
    runtime.shutdown_background();
    answer
}

/// `signal` polled once, which installs its handlers, and answered as a future `serve` can await.
/// `run` calls it before `connect`, whose hooks and readiness checks can take seconds: a signal
/// arriving then is held for `serve`, which shuts the started app down, rather than ending the
/// process by the signal's default action with no hook run.
fn installed(signal: impl Future<Output = Signal> + Send + 'static) -> impl Future<Output = Signal> + Send + 'static {
    let mut signal = Box::pin(signal);
    let early = match signal.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(signal) => Some(signal),
        Poll::Pending => None,
    };
    async move {
        match early {
            Some(signal) => signal,
            None => signal.await,
        }
    }
}

/// How [`run`] ended, when it did not end in a clean shutdown. `Display` is the wrapped error's
/// text and `Debug` is `Display`, so a `main` returning it prints the report.
#[non_exhaustive]
pub enum RunError {
    /// The tokio runtime could not be built: the OS refused a thread or the I/O driver.
    Runtime(io::Error),
    /// `wire`, `connect` or `listen` refused the app, and nothing served.
    Startup(StartupError),
    /// The app served, and its shutdown recorded failures.
    Shutdown(ShutdownError),
}

impl From<StartupError> for RunError {
    fn from(error: StartupError) -> RunError {
        RunError::Startup(error)
    }
}

impl From<ShutdownError> for RunError {
    fn from(error: ShutdownError) -> RunError {
        RunError::Shutdown(error)
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::Runtime(error) => write!(f, "the tokio runtime could not be built: {error}"),
            RunError::Startup(error) => fmt::Display::fmt(error, f),
            RunError::Shutdown(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl fmt::Debug for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Transparent for the two errors it holds unchanged: their text is its text, so their source is
/// its source, and a chain reporter prints neither twice.
impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RunError::Runtime(error) => Some(error),
            RunError::Startup(error) => error.source(),
            RunError::Shutdown(error) => error.source(),
        }
    }
}

/// What [`run`] binds: one [`Server`], or a tuple of two to eight, queued on the app in the order
/// written, as a chain of `bind` calls queues them.
///
/// `M` is [`One`] or [`Several`], picked by inference and never written. One blanket over every
/// `Server` and one over tuples of them cannot both implement a single `Servers`, since `ulo` may
/// implement `Server` for a tuple; the marker keeps the two apart.
#[diagnostic::on_unimplemented(
    message = "`{Self}` is not a server or a tuple of two to eight servers",
    label = "`run` takes one `ulo::Server`, or a tuple of two to eight",
    note = "each element of a tuple implements `ulo::Server`; more than eight are bound with the chain, one `bind` each"
)]
pub trait Servers<M> {
    fn bind_to(self, app: App<Connected>) -> App<Connected>;
}

/// The [`Servers`] marker of one server.
pub enum One {}

/// The [`Servers`] marker of a tuple of servers.
pub enum Several {}

impl<S: Server> Servers<One> for S {
    fn bind_to(self, app: App<Connected>) -> App<Connected> {
        app.bind(self)
    }
}

macro_rules! servers_tuple {
    ($($server:ident),+) => {
        impl<$($server: Server),+> Servers<Several> for ($($server,)+) {
            #[allow(non_snake_case, reason = "each binding is named for its type parameter")]
            fn bind_to(self, app: App<Connected>) -> App<Connected> {
                let ($($server,)+) = self;
                app$(.bind($server))+
            }
        }
    };
}

servers_tuple!(A, B);
servers_tuple!(A, B, C);
servers_tuple!(A, B, C, D);
servers_tuple!(A, B, C, D, E);
servers_tuple!(A, B, C, D, E, F);
servers_tuple!(A, B, C, D, E, F, G);
servers_tuple!(A, B, C, D, E, F, G, H);
