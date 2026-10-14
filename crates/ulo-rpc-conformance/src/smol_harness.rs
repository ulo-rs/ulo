//! The runtime half of a link on smol.

use std::future::Future;
use std::sync::Arc;

use ulo_runtime_smol::{Executor, Smol};

use crate::Harness;

/// The threads running a scenario's executor beside the test's own.
const WORKERS: usize = 3;

/// smol as a broker's [`Harness`]: each scenario on an `async-executor` `Executor` of its own, run
/// by the test's thread and three more through `Smol::block_on`, so a task may be polled on
/// another thread than the one that spawned it, and finds the runtime there too. No tokio runtime
/// runs.
pub struct OnSmol;

impl Harness for OnSmol {
    type Runtime = Smol;

    fn runtime() -> Smol {
        Smol::current().expect("`runtime()` is called inside `block_on`, on a thread running the scenario's executor")
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        Smol::new(Arc::new(Executor::new())).with_workers(WORKERS).block_on(fut)
    }
}
