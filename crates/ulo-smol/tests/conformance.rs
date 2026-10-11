//! The runtime conformance suite over `ulo_smol::Smol`, each scenario on an executor of its own,
//! run by the test's thread and three more through `Smol::block_on`, so a task may be polled on
//! another thread than the one aborting or awaiting it.

use std::future::Future;
use std::sync::Arc;

use ulo_runtime_conformance::{Harness, runtime_suite};
use ulo_smol::{Executor, Smol};

/// The threads running the scenario's executor beside the test's own.
const WORKERS: usize = 3;

struct OnSmol;

impl Harness for OnSmol {
    type Runtime = Smol;

    fn runtime() -> Smol {
        Smol::current().expect("`runtime()` is called inside `block_on`, on a thread running the scenario's executor")
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        Smol::new(Arc::new(Executor::new())).with_workers(WORKERS).block_on(fut)
    }
}

runtime_suite!(OnSmol);
