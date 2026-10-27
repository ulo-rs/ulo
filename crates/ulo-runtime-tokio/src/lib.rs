//! The tokio runtime for `ulo` (transports DESIGN §8): the core's [`Runtime`](ulo::Runtime) as
//! [`Tokio`], its clock alone as [`Timer`], the OS signal future [`shutdown_signal`] for `serve`,
//! and [`spawn`] and [`spawn_in`]. A tokio-based RPC link holds a `Tokio` of its own and runs its
//! I/O there through [`Tokio::run`].
//!
//! ```ignore
//! let app = App::builder(AppModule).runtime(ulo_runtime_tokio::Tokio::current()).wire()?;
//! // ..
//! app.serve(ulo_runtime_tokio::shutdown_signal()).await?;
//! ```

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use ulo::{BoxFuture, ExecutionRef, Refused, RuntimeTask, Signal, TaskHandle};

/// The app's runtime on one tokio runtime, held by its `Handle`: tasks are spawned on that
/// runtime, and sleeps registered with its timer, from whichever thread asks, a worker of another
/// runtime or a plain thread with none, as a client's drop may run on. Set with
/// `AppBuilder::runtime(ulo_runtime_tokio::Tokio::current())`, or given to a client built outside an app.
/// A tokio-based RPC link holds one of its own, whatever runtime the app was given.
///
/// A runtime that has shut down refuses, as a value and never a panic: `try_spawn` and
/// `try_sleep` answer `Err(Refused)`, `spawn` drops the task unrun and its handle answers
/// `Aborted`, a sleep ends at once, the shutdown falling before the sleep or during it, and `now`
/// still answers.
#[derive(Clone, Debug)]
pub struct Tokio {
    handle: Handle,
}

impl Tokio {
    /// The tokio runtime current on the calling thread. Panics outside one, at this call, as
    /// `Handle::current` does; nothing after it looks for a runtime.
    pub fn current() -> Tokio {
        Tokio { handle: Handle::current() }
    }

    pub fn from_handle(handle: Handle) -> Tokio {
        Tokio { handle }
    }

    /// The tokio runtime current on the calling thread, `None` outside one.
    pub fn try_current() -> Option<Tokio> {
        Handle::try_current().ok().map(|handle| Tokio { handle })
    }

    pub fn handle(&self) -> &Handle {
        &self.handle
    }

    /// `fut` run as a task on the held runtime, its output answered to whichever executor awaits
    /// the returned future: a tokio worker, another runtime's, or a plain thread with none. What a
    /// tokio-based RPC link does with tokio's reactor, timer or spawner goes through here, so the
    /// link's futures do not depend on the caller's executor.
    ///
    /// The task is spawned when the answer is first polled, so answers created in order and
    /// awaited in order run in order. Dropping the answer aborts the task. A panic in the task
    /// resumes where the answer is awaited. A runtime that has shut down drops the task unrun,
    /// answered as `Err(Stopped)`.
    pub fn run<F>(&self, fut: F) -> impl Future<Output = Result<F::Output, Stopped>> + Send + 'static
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let handle = self.handle.clone();
        async move {
            match Aborting(handle.spawn(fut)).await {
                Ok(output) => Ok(output),
                Err(error) if error.is_panic() => std::panic::resume_unwind(error.into_panic()),
                Err(_) => Err(Stopped),
            }
        }
    }
}

/// [`Tokio::run`]'s answer when the runtime it holds has shut down before the task finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stopped;

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the tokio runtime the task was spawned on has shut down")
    }
}

impl std::error::Error for Stopped {}

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

/// `now` enters the held runtime, so it reads that runtime's clock, a paused one included,
/// wherever it is called.
impl ulo::Timer for Tokio {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        let sleep = ulo::Timer::try_sleep(self, d);
        Box::pin(async move {
            let _ = sleep.await;
        })
    }

    fn now(&self) -> Instant {
        let _entered = self.handle.enter();
        ulo::Timer::now(&Timer)
    }

    /// The sleep runs as a task of the held runtime. tokio's timer panics when a sleep is polled
    /// after the runtime has shut down, wherever it is polled, and a runtime drops its tasks
    /// before it shuts its timer down, so a sleep inside one is never polled then: the runtime
    /// drops it and the task's `JoinHandle` reports that. The deadline is read here, as
    /// `tokio::time::sleep` reads it at its call; the task is spawned at the first poll.
    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        let deadline = {
            let _entered = self.handle.enter();
            let now = tokio::time::Instant::now();
            // `tokio::time::sleep`'s own answer to a duration past what an `Instant` holds.
            now.checked_add(d).unwrap_or_else(|| now + Duration::from_secs(86400 * 365 * 30))
        };
        let handle = self.handle.clone();
        Box::pin(async move {
            // The task's future cannot panic, so an error is the runtime dropping it.
            Aborting(handle.spawn(async move { tokio::time::sleep_until(deadline).await })).await.map_err(|_| Refused)
        })
    }
}

impl ulo::Spawn for Tokio {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        TaskHandle::launch(fut, |task| Task(Some(self.handle.spawn(task))))
    }

    /// tokio offers no check for a runtime that has shut down; one drops a task inside `spawn`,
    /// so the task's `JoinHandle` answers on its first poll. The wrapper the core spawns cannot
    /// panic and nothing has aborted the task yet, so an error there is that drop.
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

/// The app's clock on tokio: sleeps through `tokio::time`, and `now()` through
/// `tokio::time::Instant::now().into_std()`, so a paused test clock (`tokio::time::pause`) drives
/// deadlines and sleeps together, as the core requires of one `Timer`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timer;

impl ulo::Timer for Timer {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(d))
    }

    fn now(&self) -> Instant {
        tokio::time::Instant::now().into_std()
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
