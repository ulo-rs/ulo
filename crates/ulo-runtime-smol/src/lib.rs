//! The smol runtime for `ulo` (transports DESIGN §8): [`run`], which serves an application on an
//! executor it builds until the OS asks the process to stop; the core's
//! [`Runtime`](ulo::Runtime) as [`Smol`], spawning on an `async-executor` [`Executor`] the caller
//! owns and runs, and timing with `async-io`'s timers; the OS signal future
//! [`shutdown_signal`] for `serve`; and [`testing::start`], which serves a `TestApp` on a socket. No
//! tokio in its tree.
//!
//! ```no_run
//! # #[ulo::module]
//! # struct AppModule;
//! # fn main_with(server: impl ulo::Server) -> Result<(), Box<dyn std::error::Error>> {
//! // `server` is a transport's, `ulo_http_hyper::ServerOn::<SmolListener>::new("127.0.0.1:3000")`
//! // for one.
//! ulo_runtime_smol::run(AppModule, server)?;
//! # Ok(())
//! # }
//! ```
//!
//! An application that sets a builder knob, reads the app between two phases or ends on a future
//! of its own writes the phases `run` performs, on an executor it runs itself:
//!
//! ```
//! # use std::sync::Arc;
//! # use ulo::{App, Server};
//! # #[ulo::module]
//! # struct AppModule;
//! # fn f(server: impl Server) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let smol = ulo_runtime_smol::Smol::new(Arc::new(ulo_runtime_smol::Executor::new()));
//! let app = App::builder(AppModule).runtime(smol.clone()).wire()?;
//! // `serve` runs inside the executor, which runs every task the app spawns while it waits.
//! smol.block_on(async {
//!     let app = app.connect().await?.bind(server).listen().await?;
//!     app.serve(ulo_runtime_smol::shutdown_signal()).await?;
//!     Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
//! })?;
//! # Ok(())
//! # }
//! ```
//!
//! [`Smol::block_on`] is also how a test drives a smol app: with
//! [`with_workers`](Smol::with_workers), the executor runs on threads beside the test's, so a task
//! may be polled on another thread than the one that spawned it, and [`Smol::current`] answers the
//! runtime on each of them.

pub mod testing;

use std::cell::RefCell;
use std::error::Error;
use std::fmt;
use std::future::{Future, poll_fn};
use std::io::{self, Write};
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

pub use async_executor::Executor;
use async_executor::FallibleTask;
use event_listener::Event;
use tracing::subscriber::NoSubscriber;
use ulo::{App, BoundAddr, BoxFuture, Module, Refused, RuntimeTask, Servers, Shutdown, ShutdownError, Signal, StartupError, TaskHandle};

/// The app's runtime on one `async-executor` [`Executor`]: tasks are spawned on it from whichever
/// thread asks, and run on the threads that run it (`executor.run(..)`, smol's own pattern), so a
/// spawned task makes progress only while some thread does. Sleeps are `async-io` timers, which
/// `async-io`'s own reactor thread fires whichever executor waits on them. Set with
/// `AppBuilder::runtime(ulo_runtime_smol::Smol::new(executor))`, or given to a client built outside an
/// app.
///
/// The executor is the caller's, not a global one: the caller decides how many threads run it and
/// when they stop. A task spawned on an executor nothing runs again is never polled, and an
/// executor dropped with tasks in it drops them, each handle answering `Aborted`.
///
/// A `Smol` never refuses to spawn or to time, so its `try_spawn` and `try_sleep` answer `Ok`: it
/// holds its executor, which `async-executor` gives no way to shut down, and `async-io`'s timers
/// fire from a reactor thread that lives as long as the process.
#[derive(Clone)]
pub struct Smol {
    executor: Arc<Executor<'static>>,
    workers: usize,
}

thread_local! {
    /// The runtime whose `block_on` runs on this thread, as its caller or as one of its workers.
    static CURRENT: RefCell<Option<Smol>> = const { RefCell::new(None) };
}

impl Smol {
    pub fn new(executor: Arc<Executor<'static>>) -> Smol {
        Smol { executor, workers: 0 }
    }

    pub fn executor(&self) -> &Arc<Executor<'static>> {
        &self.executor
    }

    /// The threads [`block_on`](Self::block_on) runs the executor on beside its caller; none
    /// unset. Spawning and timing are unaffected.
    pub fn with_workers(self, workers: usize) -> Smol {
        Smol { workers, ..self }
    }

    /// Blocks the calling thread until `fut` completes, running the executor there while it waits,
    /// and on the [`with_workers`](Self::with_workers) threads, which start here and are joined
    /// before it returns, `fut` panicking included. A task spawned on the runtime makes progress
    /// while `fut` runs; one still pending when `fut` completes stays on the executor, polled
    /// again by whatever runs it next.
    ///
    /// Inside it, [`current`](Self::current) answers this runtime on the calling thread and on
    /// every worker, so code that finds its runtime by thread reaches it from a spawned task too.
    /// For an application's `main`, and for a test driving a smol app:
    ///
    /// ```
    /// use std::sync::Arc;
    /// use ulo::Spawn;
    /// use ulo_runtime_smol::{Executor, Smol};
    ///
    /// let smol = Smol::new(Arc::new(Executor::new())).with_workers(3);
    /// let answer = smol.block_on(async {
    ///     let runtime = Smol::current().expect("inside `block_on`");
    ///     ulo::spawn_with(&runtime, async { 41 + 1 }).await
    /// });
    /// assert_eq!(answer.ok(), Some(42));
    /// ```
    ///
    /// A worker whose task panicked panics this call once `fut` has completed, unless `fut`
    /// itself is panicking.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        let stop = Arc::new(Stop { stopped: AtomicBool::new(false), event: Event::new() });
        let workers = (0..self.workers)
            .map(|_| {
                let (smol, stop) = (self.clone(), Arc::clone(&stop));
                std::thread::spawn(move || {
                    let _current = Current::enter(smol.clone());
                    async_io::block_on(smol.executor.run(stop.wait()));
                })
            })
            .collect();
        let _running = Running { stop, workers };
        let _current = Current::enter(self.clone());
        async_io::block_on(self.executor.run(fut))
    }

    /// The runtime whose [`block_on`](Self::block_on) runs on this thread, as its caller or as
    /// one of its workers; `None` outside one.
    pub fn current() -> Option<Smol> {
        CURRENT.with(|current| current.borrow().clone())
    }
}

/// This thread's [`Smol::current`] for as long as it lives, the one it replaced restored after.
struct Current(Option<Smol>);

impl Current {
    fn enter(smol: Smol) -> Current {
        Current(CURRENT.with(|current| current.borrow_mut().replace(smol)))
    }
}

impl Drop for Current {
    fn drop(&mut self) {
        let previous = self.0.take();
        CURRENT.with(|current| *current.borrow_mut() = previous);
    }
}

/// The signal that ends `block_on`'s workers.
struct Stop {
    stopped: AtomicBool,
    event: Event,
}

impl Stop {
    async fn wait(&self) {
        loop {
            if self.stopped.load(Ordering::Acquire) {
                return;
            }
            // Registered before the flag is read again, so a stop raised in between still wakes it.
            let listener = self.event.listen();
            if self.stopped.load(Ordering::Acquire) {
                return;
            }
            listener.await;
        }
    }
}

/// `block_on`'s workers, stopped and joined when it returns or unwinds.
struct Running {
    stop: Arc<Stop>,
    workers: Vec<std::thread::JoinHandle<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.stop.stopped.store(true, Ordering::Release);
        self.stop.event.notify(usize::MAX);
        let mut panicked = false;
        for worker in self.workers.drain(..) {
            panicked |= worker.join().is_err();
        }
        if panicked && !std::thread::panicking() {
            panic!("a thread running the executor for `Smol::block_on` panicked");
        }
    }
}

impl fmt::Debug for Smol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Smol").finish_non_exhaustive()
    }
}

/// `now` is the system's monotonic clock, the one `async-io` measures its timers against.
impl ulo::Timer for Smol {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            async_io::Timer::after(d).await;
        })
    }

    fn now(&self) -> Instant {
        Instant::now()
    }

    /// `sleep`, never refused: `async-io`'s reactor thread lives as long as the process.
    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        let sleep = ulo::Timer::sleep(self, d);
        Box::pin(async move {
            sleep.await;
            Ok(())
        })
    }
}

impl ulo::Spawn for Smol {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        TaskHandle::launch(fut, |task| Task::Running(self.executor.spawn(task).fallible()))
    }

    /// `spawn`, never refused: the held executor has no shutdown.
    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        Ok(ulo::Spawn::spawn(self, fut))
    }

    /// On `blocking`'s pool, `blocking::unblock`, which is smol's: one pool per process, shared
    /// by every `Smol` and outside any executor, whose threads start as work needs them, up to 500
    /// unless `BLOCKING_MAX_THREADS` sets another bound.
    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>) -> TaskHandle {
        TaskHandle::launch_blocking(work, |work| Task::Running(blocking::unblock(work).fallible()))
    }

    fn name(&self) -> &'static str {
        "smol"
    }
}

/// `async-task`'s handle as the core's `RuntimeTask`.
///
/// Dropping an `async-task` `Task` cancels it but returns at once, with the future possibly still
/// being polled on another thread or not yet dropped by the executor; `poll_ended` must not answer
/// until the future has been dropped. So `abort` takes `Task::cancel`'s future, which waits for
/// exactly that, and polls it once at once, since it marks the task cancelled on its first poll;
/// `poll_ended` then polls it to its end. `FallibleTask` answers `None` rather than panicking for
/// a task its executor dropped unrun.
enum Task {
    Running(FallibleTask<()>),
    Cancelling(Pin<Box<dyn Future<Output = Option<()>> + Send>>),
    Ended,
}

impl RuntimeTask for Task {
    fn abort(&mut self) {
        let Task::Running(task) = std::mem::replace(self, Task::Ended) else { return };
        let mut cancel: Pin<Box<dyn Future<Output = Option<()>> + Send>> = Box::pin(task.cancel());
        if cancel.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending() {
            *self = Task::Cancelling(cancel);
        }
    }

    fn poll_ended(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        let ended = match self {
            Task::Running(task) => Pin::new(task).poll(cx).map(|_| ()),
            Task::Cancelling(cancel) => cancel.as_mut().poll(cx).map(|_| ()),
            Task::Ended => Poll::Ready(()),
        };
        if ended.is_ready() {
            *self = Task::Ended;
        }
        ended
    }

    /// A cancelled task is already stopping; dropping its `cancel` future leaves it to.
    fn detach(self: Box<Self>) {
        if let Task::Running(task) = *self {
            task.detach();
        }
    }
}

/// Serves `root` on an executor of its own until the OS asks the process to stop, for a `main`
/// that starts no executor. It is the startup chain with no knob set, inside
/// [`Smol::block_on`] on a new [`Executor`] run by the calling thread and by one worker for each
/// further thread [`std::thread::available_parallelism`] reports:
///
/// ```ignore
/// # // reason: the chain `run` performs, written out; `bind_to` queues each server `servers` holds
/// smol.block_on(async {
///     // `run` installs its handlers at this point; this future installs them at its first poll.
///     let signal = shutdown_signal();
///     let app = App::builder(root).runtime(smol.clone()).wire()?.connect().await?;
///     let app = servers.bind_to(app).listen().await?;
///     app.serve(signal).await
/// })
/// ```
///
/// `servers` is one [`Server`](ulo::Server) or a tuple of two to eight, bound in the order
/// written ([`Servers`]). Each address the app listens on is logged at `info` through `tracing`,
/// as `ulo.transport` and `address`, once `listen` has bound them all; with no `tracing`
/// subscriber set, so that the event reaches nothing, `run` writes `listening on <address>
/// (<transport>)` to stderr in its place, one line per address. [`shutdown_signal`] ends it:
/// SIGINT or SIGTERM on Unix, Ctrl-C on Windows. Its handlers are installed before `wire`, so a
/// signal that arrives while `connect` runs hooks and readiness checks is held: the app finishes
/// starting and `serve` shuts it down, where the signal's default action would end the process
/// with no hook run. The app then runs its shutdown sequence, its hooks, the drain and each
/// server's `close`, and `run` answers that shutdown's [`Shutdown`], or [`RunError::Shutdown`]
/// listing its failures. A startup that fails answers [`RunError::Startup`] with nothing served,
/// whatever had initialized torn down as `connect` tears it down. The workers are joined before
/// `run` returns.
///
/// A second signal, from the first on until `run` returns, ends the process at once: `run` writes
/// `shutdown cut short by a second signal, <signal>` to stderr and exits with the status a shell
/// reports for a process that signal ended, 130 for SIGINT and 143 for SIGTERM, and on Windows
/// `STATUS_CONTROL_C_EXIT`, the status its default Ctrl-C handler ends a process with. Nothing
/// after it runs: no hook, no `close`, no destructor. The signals are read on a thread of their
/// own, not on the executor's, so a hook or a drain holding every executor thread does not hold
/// the exit. A supervisor reads the status as a failure, where a shutdown that finished answers
/// `Ok` and a `main` returning it exits 0. The handlers stay installed once `run` has returned,
/// with nothing reading them: a SIGINT or SIGTERM after that does nothing, so a `main` with work
/// left after `run` is stopped by SIGKILL alone.
///
/// An application that sets a builder knob (`drain_timeout`, `shutdown_timeout`, `unserved`, a
/// test clock), reads the app between two phases, ends on a future of its own or runs on an
/// executor it sizes writes the chain itself, inside a [`Smol::block_on`] of its own.
pub fn run(root: impl Module, servers: impl Servers) -> Result<Shutdown, RunError> {
    let workers = std::thread::available_parallelism().map_or(0, |threads| threads.get() - 1);
    let smol = Smol::new(Arc::new(Executor::new())).with_workers(workers);
    let signals = Watch::start();
    smol.block_on(async {
        let app = App::builder(root).runtime(smol.clone()).wire()?.connect().await?;
        let app = servers.bind_to(app).listen().await?;
        let unheard = tracing::dispatcher::get_default(|dispatch| dispatch.is::<NoSubscriber>());
        for bound in app.addresses() {
            tracing::info!(ulo.transport = bound.transport, address = %bound.addr, "listening");
            if unheard {
                listening(&bound);
            }
        }
        Ok(app.serve(signals.first()).await?)
    })
}

/// The OS's shutdown signals, their handlers installed at [`start`](Self::start) and read on a
/// thread of their own: the first answered by [`first`](Self::first), for `serve`, and the second
/// by [`cut_short`]. One set of handlers counts both, so no delivery reaches one and not the other.
/// Dropping it stops and joins the thread.
///
/// `run` starts it before `connect`, whose hooks and readiness checks can take seconds: a signal
/// arriving then is held for `serve`, which shuts the started app down, rather than ending the
/// process by the signal's default action with no hook run.
struct Watch {
    first: Arc<First>,
    stop: Arc<Stop>,
    thread: Option<std::thread::JoinHandle<()>>,
}

/// The first signal, set once by the watching thread.
struct First {
    signal: Mutex<Option<Signal>>,
    event: Event,
}

impl Watch {
    fn start() -> Watch {
        let mut signals = os::Signals::install();
        let first = Arc::new(First { signal: Mutex::new(None), event: Event::new() });
        let stop = Arc::new(Stop { stopped: AtomicBool::new(false), event: Event::new() });
        let thread = {
            let (first, stop) = (Arc::clone(&first), Arc::clone(&stop));
            std::thread::spawn(move || {
                let mut watch = pin!(async {
                    let signal = signals.next().await;
                    *first.signal.lock().unwrap_or_else(PoisonError::into_inner) = Some(signal);
                    first.event.notify(usize::MAX);
                    cut_short(&signals.next().await);
                });
                let mut stopped = pin!(stop.wait());
                async_io::block_on(poll_fn(|cx| match stopped.as_mut().poll(cx) {
                    Poll::Ready(()) => Poll::Ready(()),
                    Poll::Pending => watch.as_mut().poll(cx),
                }));
            })
        };
        Watch { first, stop, thread: Some(thread) }
    }

    async fn first(&self) -> Signal {
        loop {
            if let Some(signal) = self.first.signal.lock().unwrap_or_else(PoisonError::into_inner).clone() {
                return signal;
            }
            // Registered before the slot is read again, so a signal set in between still wakes it.
            let listener = self.first.event.listen();
            if let Some(signal) = self.first.signal.lock().unwrap_or_else(PoisonError::into_inner).clone() {
                return signal;
            }
            listener.await;
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.stopped.store(true, Ordering::Release);
        self.stop.event.notify(usize::MAX);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Ends the process on a second signal, with one line saying so and the status a shell reports for
/// a process the signal ended.
fn cut_short(signal: &Signal) -> ! {
    let status = match signal.name() {
        "SIGINT" => 130,
        "SIGTERM" => 143,
        // `STATUS_CONTROL_C_EXIT`, 0xC000013A, which Windows' default handler ends a process with
        // on Ctrl-C.
        _ => -1_073_741_510,
    };
    // One write, so a log line written from another thread cannot land inside it.
    let _ = io::stderr().write_all(format!("shutdown cut short by a second signal, {signal}\n").as_bytes());
    std::process::exit(status)
}

/// The line `run` writes for an address when no subscriber hears its `listening` event.
fn listening(bound: &BoundAddr) {
    let tls = if bound.tls { " over TLS" } else { "" };
    let _ = writeln!(io::stderr(), "listening on {} ({}{tls})", bound.addr, bound.transport);
}

/// Resolves on the first shutdown signal the OS delivers: SIGINT or SIGTERM on Unix, Ctrl-C on
/// Windows, as a `Signal` named after it (`"SIGTERM"`, `"SIGINT"`, `"CTRL_C"`). A handler that
/// cannot be installed is skipped and the others still resolve it; with none installed, or on
/// another platform, the future never resolves.
///
/// The handlers are installed when the future is first polled, through `async-signal`, whose
/// notifications `async-io`'s reactor delivers, so it resolves on any executor and the function
/// may be called anywhere. A handler stays installed for the rest of the process with no action
/// once the future has resolved or dropped, so a second SIGINT during the shutdown sequence does
/// not end it; [`run`] ends the process on one.
pub fn shutdown_signal() -> impl Future<Output = Signal> + Send + 'static {
    first_signal()
}

async fn first_signal() -> Signal {
    os::Signals::install().next().await
}

#[cfg(any(unix, windows))]
mod os {
    use std::future::poll_fn;
    use std::pin::Pin;

    use async_signal::Signal as Os;
    use futures_core::Stream;
    use ulo::Signal;

    /// Ctrl-C reaches a Windows process as `SIGINT`, the one signal `async-signal` installs there.
    const INTERRUPT: &str = if cfg!(windows) { "CTRL_C" } else { "SIGINT" };

    /// SIGTERM and SIGINT, read by [`next`](Self::next); `None` when no notifier could be made.
    pub(super) struct Signals(Option<async_signal::Signals>);

    impl Signals {
        /// A signal delivered from here on is answered by `next`.
        pub(super) fn install() -> Signals {
            let Ok(mut signals) = async_signal::Signals::new(std::iter::empty::<Os>()) else {
                return Signals(None);
            };
            // One at a time, so a signal the platform refuses, `SIGTERM` on Windows, leaves the other.
            let _ = signals.add_signals([Os::Term]);
            let _ = signals.add_signals([Os::Int]);
            Signals(Some(signals))
        }

        pub(super) async fn next(&mut self) -> Signal {
            let Some(signals) = &mut self.0 else {
                return std::future::pending().await;
            };
            match poll_fn(|cx| Pin::new(&mut *signals).poll_next(cx)).await {
                Some(Ok(Os::Term)) => Signal::new("SIGTERM"),
                Some(Ok(_)) => Signal::new(INTERRUPT),
                // The notifier failed and no signal can arrive through it any more.
                Some(Err(_)) | None => std::future::pending().await,
            }
        }
    }
}

#[cfg(not(any(unix, windows)))]
mod os {
    use ulo::Signal;

    pub(super) struct Signals;

    impl Signals {
        pub(super) fn install() -> Signals {
            Signals
        }

        pub(super) async fn next(&mut self) -> Signal {
            std::future::pending().await
        }
    }
}

/// How [`run`] ended, when it did not end in a clean shutdown. `Display` is the wrapped error's
/// text and `Debug` is `Display`, so a `main` returning it prints the report.
#[non_exhaustive]
pub enum RunError {
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

/// Transparent: the error it holds unchanged is its text, so that error's source is its source,
/// and a chain reporter prints neither twice.
impl Error for RunError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RunError::Startup(error) => error.source(),
            RunError::Shutdown(error) => error.source(),
        }
    }
}
