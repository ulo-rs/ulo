//! The runtime conformance suite over `ulo_runtime_smol::Smol`, each scenario on an executor of its own,
//! run by the test's thread and three more through `Smol::block_on`, so a task may be polled on
//! another thread than the one aborting or awaiting it.

use std::future::Future;
use std::sync::Arc;

use ulo_runtime_conformance::{Harness, Stoppable, runtime_suite};
use ulo_runtime_smol::{Executor, Smol};

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

    fn stoppable() -> Option<Stoppable<Smol>> {
        None
    }
}

runtime_suite!(OnSmol; not_applicable {
    a_shut_down_runtime_refuses_spawning_and_timing_without_panicking:
        "smol has no runtime value that can be shut down while code still holds it",
    run_on_a_shut_down_runtime_answers_runtime_shut_down:
        "smol has no runtime value that can be shut down while code still holds it",
});
