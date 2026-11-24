//! `spawn_blocking`: work that blocks runs on threads of its own, beside the runtime's tasks, and
//! its handle answers as a task's does.

use std::collections::HashSet;
use std::future::Future;
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::thread::{self, ThreadId};
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use futures::{FutureExt, StreamExt};
use ulo::{Runtime, TaskEnd, spawn_blocking_with};

use super::{PANIC_MESSAGE, PATIENCE, assert_panicked_with_the_message, within};

/// The fewest pieces of blocking work [`beside_tasks`] holds at once, whatever the runtime's
/// worker count.
pub const MIN_HELD_AT_ONCE: usize = 4;

/// How many pieces of blocking work [`beside_tasks`] holds at once on a runtime whose tasks
/// `workers` threads poll: one more than `workers`, so a runtime running blocking work on those
/// threads has none left for the task that releases them, and at least [`MIN_HELD_AT_ONCE`].
/// Outnumbering the workers is all the property needs, that blocking work does not stall the
/// runtime's tasks, so a runtime whose blocking pool runs this many at once passes.
pub fn held_at_once(workers: usize) -> usize {
    workers.saturating_add(1).max(MIN_HELD_AT_ONCE)
}

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

/// [`held_at_once`]`(workers)` pieces of blocking work each wait for a release a task spawned on
/// the runtime gives only once every piece has begun, and every piece receives it: the runtime ran
/// them on threads that do not poll its tasks, all at once, and the task ran meanwhile. Each ends
/// `Finished`. `workers` is [`Harness::workers`](crate::Harness::workers).
pub async fn beside_tasks<R: Runtime>(rt: R, workers: usize) {
    let held = held_at_once(workers);
    let release = Release::new();
    let (began, begun) = mpsc::unbounded::<()>();
    let received = Arc::new(Mutex::new(0_usize));
    let mut handles = Vec::with_capacity(held);
    for _ in 0..held {
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
            let _ = begun.take(held).count().await;
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
        received, held,
        "of {held} pieces of blocking work held at once, more than the {workers} threads polling tasks, {received} received the release a task spawned on the runtime gave: the runtime ran blocking work where its tasks run, or not all at once"
    );
}

/// The fewest tasks [`polled_within_workers`] spawns, whatever the machine.
const MIN_SPREAD: usize = 64;

/// How many tasks [`polled_within_workers`] spawns on a runtime whose harness answers `workers`:
/// four for each of the machine's cores or of `workers + 1`, whichever is more, and at least
/// [`MIN_SPREAD`], so a runtime has more pending at once than it has threads to poll them.
fn spread(workers: usize) -> usize {
    let cores = thread::available_parallelism().map_or(1, NonZeroUsize::get);
    cores.max(workers.saturating_add(1)).saturating_mul(4).max(MIN_SPREAD)
}

/// How many times each task of [`polled_within_workers`] yields once the gate opens, each yield a
/// further poll whose thread is recorded.
const YIELDS: usize = 3;

/// How long each task of [`polled_within_workers`] holds its thread at its first poll, so a runtime
/// hands the tasks still queued to its other threads rather than polling them all on one. The
/// verdict reads only which threads polled, so the hold widens what the scenario observes and
/// decides nothing.
const HOLD: Duration = Duration::from_millis(1);

/// Tasks spawned on the runtime are polled on no more distinct threads than `workers` says poll
/// its tasks. The scenario spawns four for each of the machine's cores or of `workers + 1`,
/// whichever is more, and at least 64. Every poll of every task records its thread; each task
/// holds its thread for a millisecond at its first poll, so the runtime hands the others to its
/// other threads, then waits for a gate the scenario opens only after all of them have begun, so
/// all are pending at once, and then yields three times. The count is of the distinct threads over
/// the whole scenario, so it is never more than a truthful `workers` on a runtime that keeps the
/// same threads polling its tasks meanwhile; a runtime that hands a worker's place to a new
/// thread, as tokio's `block_in_place` does, has each thread counted. A harness answering fewer
/// threads fails once the runtime has spread its tasks past that many. `workers`
/// is [`Harness::workers`](crate::Harness::workers), which sizes [`beside_tasks`].
pub async fn polled_within_workers<R: Runtime>(rt: R, workers: usize) {
    let tasks = spread(workers);
    let polled = Arc::new(Mutex::new(HashSet::new()));
    let (began, begun) = mpsc::unbounded::<()>();
    let (open, gate) = oneshot::channel::<()>();
    let gate = gate.shared();
    let handles: Vec<_> = (0..tasks)
        .map(|_| {
            let (began, gate) = (began.clone(), gate.clone());
            rt.spawn(Box::pin(Recording {
                polled: Arc::clone(&polled),
                fut: Box::pin(async move {
                    thread::sleep(HOLD);
                    let _ = began.unbounded_send(());
                    let _ = gate.await;
                    for _ in 0..YIELDS {
                        YieldNow(false).await;
                    }
                }),
            }))
        })
        .collect();
    drop(began);
    within(&rt, begun.take(tasks).count()).await.expect("the tasks spawned did not all begin");
    let _ = open.send(());
    for handle in handles {
        let end = within(&rt, handle).await.expect("a task waiting on the opened gate did not end");
        assert!(end.is_finished(), "a task that returned ended {end:?}");
    }
    let polled = polled.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(failure) = undercount(polled.iter(), workers) {
        panic!("{failure}");
    }
}

/// The failure [`polled_within_workers`] reports when `polled`, the threads recorded polling its
/// tasks, holds more distinct threads than `workers`.
fn undercount<I: Eq + Hash>(polled: impl IntoIterator<Item = I>, workers: usize) -> Option<String> {
    let threads = polled.into_iter().collect::<HashSet<_>>().len();
    (threads > workers).then(|| {
        format!(
            "tasks spawned on the runtime were polled on {threads} distinct threads, more than the {workers} `Harness::workers` answers: the harness counts fewer threads than poll the runtime's tasks, so the blocking scenario's {} pieces of blocking work need not outnumber them",
            held_at_once(workers)
        )
    })
}

/// A task's future, recording the thread of each poll in `polled`.
struct Recording<F> {
    polled: Arc<Mutex<HashSet<ThreadId>>>,
    fut: Pin<Box<F>>,
}

impl<F: Future> Future for Recording<F> {
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let this = self.get_mut();
        this.polled.lock().unwrap_or_else(PoisonError::into_inner).insert(thread::current().id());
        this.fut.as_mut().poll(cx)
    }
}

/// Pending once, waking itself, then ready.
struct YieldNow(bool);

impl Future for YieldNow {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
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

#[cfg(test)]
mod tests {
    use super::{held_at_once, undercount};

    #[test]
    fn as_many_threads_as_workers_or_fewer_pass() {
        assert_eq!(undercount([1, 2, 3, 4], 4), None);
        assert_eq!(undercount([1, 2], 4), None);
        assert_eq!(undercount(Vec::<u32>::new(), 0), None);
    }

    #[test]
    fn a_thread_recorded_at_several_polls_counts_once() {
        assert_eq!(undercount([7, 7, 7, 8, 8], 2), None);
        assert!(undercount([7, 7, 7, 8, 8, 9], 2).is_some());
    }

    #[test]
    fn more_threads_than_workers_fail_naming_both_counts() {
        let failure = undercount(0..10, 1).expect("ten threads against a claim of one");
        assert!(failure.contains("10 distinct threads"), "{failure}");
        assert!(failure.contains("the 1 `Harness::workers` answers"), "{failure}");
        assert!(failure.contains(&format!("{} pieces of blocking work", held_at_once(1))), "{failure}");
    }
}
