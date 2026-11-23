//! `spawn_blocking`: work that blocks runs on threads of its own, beside the runtime's tasks, and
//! its handle answers as a task's does.

use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread::{self, ThreadId};

use futures::StreamExt;
use futures::channel::{mpsc, oneshot};
use ulo::{Runtime, TaskEnd, spawn_blocking_with};

use super::{PANIC_MESSAGE, PATIENCE, assert_panicked_with_the_message, within};

/// How many pieces of blocking work [`beside_tasks`] holds at once: more than any executor runs
/// threads, so a runtime running blocking work on the threads that poll its tasks has none left
/// for the task that releases them. Within tokio's 512 and smol's 500.
pub const HELD_AT_ONCE: usize = 64;

/// A release every piece of blocking work waits on, bounded by [`PATIENCE`] so a runtime that
/// never lets the release run fails rather than hangs.
struct Release {
    released: Mutex<bool>,
    changed: Condvar,
}

impl Release {
    fn new() -> Arc<Release> {
        Arc::new(Release { released: Mutex::new(false), changed: Condvar::new() })
    }

    fn open(&self) {
        *self.released.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    /// Whether the release came within [`PATIENCE`].
    fn wait(&self) -> bool {
        let released = self.released.lock().unwrap_or_else(PoisonError::into_inner);
        let (released, _) = self
            .changed
            .wait_timeout_while(released, PATIENCE, |released| !*released)
            .unwrap_or_else(PoisonError::into_inner);
        *released
    }
}

/// [`HELD_AT_ONCE`] pieces of blocking work each wait for a release a task spawned on the runtime
/// gives only once every piece has begun, and every piece receives it: the runtime ran them on
/// threads that do not poll its tasks, all at once, and the task ran meanwhile. Each ends
/// `Finished`.
pub async fn beside_tasks<R: Runtime>(rt: R) {
    let release = Release::new();
    let (began, begun) = mpsc::unbounded::<()>();
    let received = Arc::new(Mutex::new(0_usize));
    let mut handles = Vec::with_capacity(HELD_AT_ONCE);
    for _ in 0..HELD_AT_ONCE {
        let (release, began, received) = (Arc::clone(&release), began.clone(), Arc::clone(&received));
        handles.push(rt.spawn_blocking(Box::new(move || {
            let _ = began.unbounded_send(());
            if release.wait() {
                *received.lock().unwrap_or_else(PoisonError::into_inner) += 1;
            }
        })));
    }
    drop(began);
    let releasing = {
        let release = Arc::clone(&release);
        rt.spawn(Box::pin(async move {
            let _ = begun.take(HELD_AT_ONCE).count().await;
            release.open();
        }))
    };
    within(&rt, releasing).await.expect("the task releasing the blocking work did not end: the pieces did not all begin, or the task did not run while they held their threads");
    for handle in handles {
        let end = within(&rt, handle).await.expect("blocking work did not end");
        assert!(end.is_finished(), "blocking work that returned ended {end:?}");
    }
    let received = *received.lock().unwrap_or_else(PoisonError::into_inner);
    assert_eq!(
        received, HELD_AT_ONCE,
        "of {HELD_AT_ONCE} pieces of blocking work held at once, {received} received the release a task spawned on the runtime gave: the runtime ran blocking work where its tasks run, or not all at once"
    );
}

/// Blocking work runs on another thread than the one that hands it over, and `spawn_blocking_with`
/// answers its value.
pub async fn answered<R: Runtime>(rt: R) {
    let caller = thread::current().id();
    let (report, ran_on) = oneshot::channel::<ThreadId>();
    let task = rt.spawn_blocking(Box::new(move || {
        let _ = report.send(thread::current().id());
    }));
    let end = within(&rt, task).await.expect("blocking work that returns at once did not end");
    assert!(end.is_finished(), "blocking work that returned ended {end:?}");
    let ran_on = within(&rt, ran_on).await.expect("blocking work reported no thread").expect("blocking work was dropped unrun");
    assert_ne!(ran_on, caller, "blocking work ran on the thread that handed it to `spawn_blocking`");
    let answer = within(&rt, spawn_blocking_with(&rt, || 7_u32)).await;
    assert!(matches!(answer, Some(Ok(7))), "the value blocking work returned through `spawn_blocking_with`: {answer:?}");
}

/// Blocking work that panics ends `Panicked` with the panic's message, its userinfo stripped, and
/// the panic goes no further; `spawn_blocking_with` answers it as `Err`.
pub async fn panicked<R: Runtime>(rt: R) {
    let task = rt.spawn_blocking(Box::new(|| panic!("{PANIC_MESSAGE}")));
    let end = within(&rt, task).await.expect("blocking work that panicked did not end");
    assert_panicked_with_the_message(&end);
    let answer = within(&rt, spawn_blocking_with(&rt, || -> u32 { panic!("{PANIC_MESSAGE}") })).await;
    match answer {
        Some(Err(end @ TaskEnd::Panicked(_))) => assert_panicked_with_the_message(&end),
        other => panic!("blocking work that panicked through `spawn_blocking_with` answered {other:?}"),
    }
}
