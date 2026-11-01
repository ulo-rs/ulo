//! `Smol::block_on`: the executor runs on the calling thread while the future does, and on the
//! workers `with_workers` names, which are stopped and joined before it returns, a panicking
//! future included; `Smol::current` answers the runtime on every one of those threads and nowhere
//! else. Each test runs on a thread of its own under [`PATIENCE`], so workers left running fail it
//! rather than hang the binary.

use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use ulo::Spawn;
use ulo_runtime_smol::{Executor, Smol};

/// How long a test, `block_on` and its workers' join included, may take.
const PATIENCE: Duration = Duration::from_secs(5);

fn smol(workers: usize) -> Smol {
    Smol::new(Arc::new(Executor::new())).with_workers(workers)
}

/// `test`'s output, run on a thread of its own; a panic in it is this call's.
fn bounded<T: Send + 'static>(test: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, finished) = mpsc::channel();
    let running = thread::spawn(move || {
        let _ = done.send(test());
    });
    match finished.recv_timeout(PATIENCE) {
        Ok(output) => output,
        Err(RecvTimeoutError::Timeout) => panic!("the test did not finish within {PATIENCE:?}"),
        Err(RecvTimeoutError::Disconnected) => match running.join() {
            Err(panic) => resume_unwind(panic),
            Ok(()) => unreachable!("the test's thread ended without sending its output"),
        },
    }
}

#[test]
fn a_task_spawned_inside_runs_while_the_future_waits_for_it() {
    let answer = bounded(|| {
        let smol = smol(0);
        smol.block_on(ulo::spawn_with(&smol, async { 41 + 1 })).ok()
    });
    assert_eq!(answer, Some(42), "the spawned task's answer");
}

#[test]
fn workers_poll_tasks_on_threads_beside_the_caller() {
    bounded(workers_poll_tasks);
}

fn workers_poll_tasks() {
    let smol = smol(3);
    let caller = thread::current().id();
    let ran_on: Arc<Mutex<HashSet<ThreadId>>> = Arc::default();
    smol.block_on(async {
        let tasks: Vec<_> = (0..12)
            .map(|_| {
                let ran_on = Arc::clone(&ran_on);
                ulo::spawn_with(&smol, async move {
                    ran_on.lock().expect("the record's lock is not poisoned").insert(thread::current().id());
                })
            })
            .collect();
        // Blocks the calling thread, so only a worker can run the tasks meanwhile.
        thread::sleep(Duration::from_millis(200));
        for task in tasks {
            let _ = task.await;
        }
    });
    let ran_on = ran_on.lock().expect("the record's lock is not poisoned");
    assert!(ran_on.iter().any(|thread| *thread != caller), "every task ran on the caller's thread: {ran_on:?}");
}

#[test]
fn current_answers_the_runtime_on_the_caller_and_on_every_worker() {
    bounded(current_answers);
}

fn current_answers() {
    let smol = smol(3);
    assert!(Smol::current().is_none(), "a runtime is current before `block_on`");
    let seen = Arc::new(AtomicUsize::new(0));
    smol.block_on(async {
        let here = Smol::current().expect("the caller's thread has the runtime current");
        assert!(Arc::ptr_eq(here.executor(), smol.executor()), "the caller's current runtime is another executor's");
        let tasks: Vec<_> = (0..12)
            .map(|_| {
                let (seen, executor) = (Arc::clone(&seen), Arc::clone(smol.executor()));
                ulo::spawn_with(&smol, async move {
                    let current = Smol::current().expect("a thread running the executor has the runtime current");
                    assert!(Arc::ptr_eq(current.executor(), &executor), "a worker's current runtime is another executor's");
                    seen.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();
        thread::sleep(Duration::from_millis(200));
        for task in tasks {
            task.await.expect("a task found the runtime current");
        }
    });
    assert_eq!(seen.load(Ordering::SeqCst), 12, "the tasks that checked");
    assert!(Smol::current().is_none(), "the runtime stayed current after `block_on` returned");
}

#[test]
fn a_panicking_future_stops_the_workers_and_leaves_no_runtime_current() {
    let (panicked, cleared) = bounded(|| {
        let smol = smol(2);
        let panicked = catch_unwind(AssertUnwindSafe(|| smol.block_on(async { panic!("the future panics") })));
        (panicked.is_err(), Smol::current().is_none())
    });
    assert!(panicked, "the future's panic reached the caller");
    assert!(cleared, "the runtime stayed current after `block_on` unwound");
}

#[test]
fn workers_stop_when_the_future_completes() {
    let answer = bounded(|| {
        let smol = smol(3);
        // A task still pending when the future completes stays on the executor and holds no worker.
        drop(smol.spawn(Box::pin(std::future::pending::<()>())));
        smol.block_on(async { 7 })
    });
    assert_eq!(answer, 7);
}
