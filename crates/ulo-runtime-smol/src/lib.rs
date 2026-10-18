//! The smol runtime for `ulo` (transports DESIGN §8): the core's [`Runtime`](ulo::Runtime) as
//! [`Smol`], spawning on an `async-executor` [`Executor`] the caller owns and runs, and timing
//! with `async-io`'s timers. No tokio in its tree.
//!
//! ```ignore
//! let smol = ulo_runtime_smol::Smol::new(Arc::new(ulo_runtime_smol::Executor::new()));
//! let app = App::builder(AppModule).runtime(smol.clone()).wire()?;
//! // `serve` runs inside the executor, which runs every task the app spawns while it waits.
//! smol.block_on(async { app.connect().await?.bind(server).listen().await?.serve(signal).await })?;
//! ```
//!
//! [`Smol::block_on`] is also how a test drives a smol app: with
//! [`with_workers`](Smol::with_workers), the executor runs on threads beside the test's, so a task
//! may be polled on another thread than the one that spawned it, and [`Smol::current`] answers the
//! runtime on each of them.

use std::cell::RefCell;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

pub use async_executor::Executor;
use async_executor::FallibleTask;
use event_listener::Event;
use ulo::{BoxFuture, RuntimeTask, TaskHandle};

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
}

impl ulo::Spawn for Smol {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        TaskHandle::launch(fut, |task| Task::Running(self.executor.spawn(task).fallible()))
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
