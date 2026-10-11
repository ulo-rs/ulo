//! The runtime half of a link on smol.

use std::cell::RefCell;
use std::future::Future;
use std::sync::Arc;

use futures_util::FutureExt;
use ulo_smol::{Executor, Smol};

use crate::Harness;

/// The threads running a scenario's executor beside the test's own.
const WORKERS: usize = 3;

thread_local! {
    /// The executor of the scenario this thread runs, for `runtime()`: set on the thread
    /// `block_on` runs and on each worker, so a task the runtime spawns finds it too.
    static EXECUTOR: RefCell<Option<Arc<Executor<'static>>>> = const { RefCell::new(None) };
}

/// smol as a broker's [`Harness`]: each scenario on an `async-executor` `Executor` of its own, run
/// by the test's thread and three more, so a task may be polled on another thread than the one
/// that spawned it. No tokio runtime runs.
pub struct OnSmol;

impl Harness for OnSmol {
    type Runtime = Smol;

    fn runtime() -> Smol {
        let executor = EXECUTOR
            .with(|current| current.borrow().clone())
            .expect("`runtime()` is called inside `block_on`, on a thread running the scenario's executor");
        Smol::new(executor)
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        let executor = Arc::new(Executor::new());
        let (stop, stopped) = futures_channel::oneshot::channel::<()>();
        let stopped = stopped.shared();
        let workers: Vec<_> = (0..WORKERS)
            .map(|_| {
                let executor = Arc::clone(&executor);
                let stopped = stopped.clone();
                std::thread::spawn(move || {
                    EXECUTOR.with(|current| *current.borrow_mut() = Some(Arc::clone(&executor)));
                    async_io::block_on(executor.run(stopped))
                })
            })
            .collect();
        EXECUTOR.with(|current| *current.borrow_mut() = Some(Arc::clone(&executor)));
        let output = async_io::block_on(executor.run(fut));
        EXECUTOR.with(|current| *current.borrow_mut() = None);
        drop(stop);
        for worker in workers {
            let _ = worker.join().expect("a worker running the scenario's executor panicked");
        }
        output
    }
}
