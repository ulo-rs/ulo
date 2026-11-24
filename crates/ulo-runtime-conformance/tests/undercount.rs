//! The worker-count scenario run against a harness that answers fewer threads than poll the
//! runtime's tasks: smol's executor run by the test's thread and three workers, against an answer
//! of none and of one.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use ulo_runtime_conformance::cases::blocking::polled_within_workers;
use ulo_runtime_smol::{Executor, Smol};

/// The scenario's failure on four threads polling, against `answered`.
fn failure(answered: usize) -> String {
    let run = catch_unwind(AssertUnwindSafe(|| {
        Smol::new(Arc::new(Executor::new())).with_workers(3).block_on(async {
            let smol = Smol::current().expect("`block_on` makes its runtime current");
            polled_within_workers(smol, answered).await;
        });
    }));
    let panic = run.expect_err("the scenario passed a harness answering fewer threads than poll its tasks");
    panic.downcast_ref::<String>().cloned().expect("the scenario fails with its message")
}

/// Every task is polled on some thread, so an answer of none fails whichever threads the runtime
/// spreads the tasks over.
#[test]
fn an_answer_of_no_threads_fails() {
    let failure = failure(0);
    assert!(failure.contains("more than the 0 `Harness::workers` answers"), "{failure}");
}

#[test]
fn an_answer_of_one_thread_where_four_poll_fails() {
    let failure = failure(1);
    assert!(failure.contains("more than the 1 `Harness::workers` answers"), "{failure}");
}
