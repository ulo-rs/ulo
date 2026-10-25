//! One more poll for a server's tasks, given by its `close` before it aborts them.
//!
//! The core's drain ends by cancelling every execution still live, and a transport renders that
//! cancellation from the task serving the call: an HTTP 503, a gRPC UNAVAILABLE, a WebSocket 1001
//! close, an RPC `unavailable`. Writing it needs no wait, only a poll of that task. The core runs
//! from the cancellation to each server's `close` without yielding, and under a spent shutdown cap
//! it polls each `close` once and drops it, so a task woken by the cancellation can be aborted
//! before the runtime polls it. [`Turns::give`], called from the start of `close`, polls every
//! tracked task once on the calling thread, which writes those answers in `close`'s first poll.

use std::any::Any;
use std::collections::BTreeMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, TryLockError, Weak};
use std::task::{Context, Poll, Waker};

type Work = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The tasks of one server that its `close` gives a turn: each future wrapped by
/// [`track`](Self::track) before it is spawned. Cheap to clone; every clone is the same set.
///
/// ```ignore
/// let turns = Turns::default();
/// tasks.spawn(turns.track(serve_connection(io)));
/// // `close`, before any wait
/// turns.give();
/// ```
#[derive(Clone, Default)]
pub struct Turns {
    registry: Arc<Mutex<Registry>>,
}

#[derive(Default)]
struct Registry {
    next: u64,
    /// In the order the futures were tracked.
    slots: BTreeMap<u64, Weak<Slot>>,
}

impl Turns {
    /// `fut` as a future to spawn in its place: it runs `fut`, which [`give`](Self::give) can also
    /// poll while it runs.
    pub fn track<F>(&self, fut: F) -> Turn
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let slot = Arc::new(Slot {
            state: Mutex::new(State { run: Run::Running(Box::pin(fut)), waker: None }),
            again: AtomicBool::new(false),
        });
        let key = {
            let mut registry = lock(&self.registry);
            let key = registry.next;
            registry.next += 1;
            registry.slots.insert(key, Arc::downgrade(&slot));
            key
        };
        Turn { slot, key, registry: Arc::downgrade(&self.registry) }
    }

    /// Polls every tracked future still running once, on the calling thread, the latest tracked
    /// first: a task a connection spawned for one of its calls is polled before the connection
    /// that writes what the call answered. Each is polled with its own task's waker, so a future
    /// still pending is woken as it would have been. A future that ends here, or panics, ends its
    /// task on the task's next poll, the panic resumed there.
    ///
    /// A future its task is polling at that moment, on another thread or as the caller of this
    /// `give` (a handler awaiting its app's shutdown), is not waited for: its task polls it once
    /// more as that poll ends, so a `close` under a spent cap is never held by a poll in progress.
    pub fn give(&self) {
        let slots: Vec<Arc<Slot>> = lock(&self.registry).slots.values().rev().filter_map(Weak::upgrade).collect();
        for slot in slots {
            slot.turn();
        }
    }
}

/// A tracked future, spawned in its place.
pub struct Turn {
    slot: Arc<Slot>,
    key: u64,
    registry: Weak<Mutex<Registry>>,
}

struct Slot {
    state: Mutex<State>,
    /// Set by a turn that found the future being polled: the poll under way polls it once more.
    again: AtomicBool,
}

struct State {
    run: Run,
    /// The waker of the task running the future, from its last poll.
    waker: Option<Waker>,
}

enum Run {
    Running(Work),
    Done,
    Panicked(Box<dyn Any + Send>),
}

impl Slot {
    fn turn(&self) {
        let mut state = match self.state.try_lock() {
            Ok(state) => state,
            Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.again.store(true, Ordering::SeqCst);
                // The poll under way may have ended and read `again` before it was set; a lock
                // free now is that case, and this turn polls the future itself.
                match self.state.try_lock() {
                    Ok(state) => state,
                    Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
                    Err(TryLockError::WouldBlock) => return,
                }
            }
        };
        self.again.store(false, Ordering::SeqCst);
        let State { run, waker } = &mut *state;
        let Run::Running(work) = run else { return };
        let waker = waker.clone().unwrap_or_else(|| Waker::noop().clone());
        let ended = match catch_unwind(AssertUnwindSafe(|| work.as_mut().poll(&mut Context::from_waker(&waker)))) {
            Ok(Poll::Pending) => return,
            Ok(Poll::Ready(())) => Run::Done,
            Err(payload) => Run::Panicked(payload),
        };
        let finished = std::mem::replace(run, ended);
        drop(state);
        // A future that panicked may panic again in its drop.
        let _ = catch_unwind(AssertUnwindSafe(move || drop(finished)));
        waker.wake();
    }
}

impl Future for Turn {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        // One more poll at most, so a future that gives the turns on every poll still returns.
        let mut once_more = true;
        loop {
            let mut state = lock(&self.slot.state);
            let State { run, waker } = &mut *state;
            let Run::Running(work) = run else { return Self::ended(state) };
            if !waker.as_ref().is_some_and(|known| known.will_wake(cx.waker())) {
                *waker = Some(cx.waker().clone());
            }
            if work.as_mut().poll(cx).is_ready() {
                let finished = std::mem::replace(run, Run::Done);
                drop(state);
                drop(finished);
                return Poll::Ready(());
            }
            drop(state);
            // A turn found this poll under way: poll once more, now that it has seen the
            // cancellation the turn was given for.
            if !(once_more && self.slot.again.swap(false, Ordering::SeqCst)) {
                return Poll::Pending;
            }
            once_more = false;
        }
    }
}

impl Turn {
    /// A poll of a future that has ended, in a turn or in an earlier poll.
    fn ended(mut state: MutexGuard<'_, State>) -> Poll<()> {
        match &mut state.run {
            Run::Running(_) => unreachable!("an ended future is not running"),
            Run::Done => Poll::Ready(()),
            Run::Panicked(_) => {
                let Run::Panicked(payload) = std::mem::replace(&mut state.run, Run::Done) else { unreachable!() };
                drop(state);
                resume_unwind(payload)
            }
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        if let Some(registry) = self.registry.upgrade() {
            lock(&registry).slots.remove(&self.key);
        }
        // Dropped outside the lock: the future's drop may reach this set again.
        let run = std::mem::replace(&mut lock(&self.slot.state).run, Run::Done);
        drop(run);
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::Wake;

    use super::*;

    struct Count(AtomicUsize);

    impl Wake for Count {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Pending until `ready` is set; records each poll's order into `polls` under `name`.
    fn work(name: &'static str, ready: Arc<Mutex<bool>>, polls: Arc<Mutex<Vec<&'static str>>>) -> impl Future<Output = ()> + Send {
        std::future::poll_fn(move |_| {
            polls.lock().unwrap().push(name);
            if *ready.lock().unwrap() { Poll::Ready(()) } else { Poll::Pending }
        })
    }

    #[test]
    fn a_turn_polls_the_latest_tracked_first_and_ends_a_finished_task_on_its_next_poll() {
        let turns = Turns::default();
        let polls = Arc::new(Mutex::new(Vec::new()));
        let ready = Arc::new(Mutex::new(false));
        let mut first = Box::pin(turns.track(work("connection", Arc::clone(&ready), Arc::clone(&polls))));
        let mut second = Box::pin(turns.track(work("call", Arc::clone(&ready), Arc::clone(&polls))));
        let woken = Arc::new(Count(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&woken));
        let mut cx = Context::from_waker(&waker);
        assert!(first.as_mut().poll(&mut cx).is_pending());
        assert!(second.as_mut().poll(&mut cx).is_pending());
        polls.lock().unwrap().clear();

        *ready.lock().unwrap() = true;
        turns.give();
        assert_eq!(*polls.lock().unwrap(), vec!["call", "connection"], "the latest tracked is polled first");
        assert_eq!(woken.0.load(Ordering::SeqCst), 2, "each task finished by the turn is woken to end");
        assert!(first.as_mut().poll(&mut cx).is_ready());
        assert!(second.as_mut().poll(&mut cx).is_ready());
        assert_eq!(polls.lock().unwrap().len(), 2, "a future the turn ended is not polled again");
    }

    #[test]
    fn a_task_giving_the_turns_from_its_own_poll_is_polled_again_after_it_and_the_others_get_theirs() {
        let (done, finished) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let turns = Turns::default();
            let polls = Arc::new(Mutex::new(Vec::new()));
            let mut other = Box::pin(turns.track(work("other", Arc::new(Mutex::new(false)), Arc::clone(&polls))));
            let (inner, giver_polls) = (turns.clone(), Arc::clone(&polls));
            let mut giver = Box::pin(turns.track(std::future::poll_fn(move |_| {
                giver_polls.lock().unwrap().push("giver");
                inner.give();
                Poll::<()>::Pending
            })));
            let waker = Waker::noop();
            let mut cx = Context::from_waker(waker);
            assert!(other.as_mut().poll(&mut cx).is_pending());
            polls.lock().unwrap().clear();
            assert!(giver.as_mut().poll(&mut cx).is_pending());
            let _ = done.send(polls.lock().unwrap().clone());
        });
        let polls = finished.recv_timeout(std::time::Duration::from_secs(5)).expect("a give from inside a tracked poll returns");
        // The giver's own turn is its second poll, which gives again and finds the other's turn.
        assert_eq!(polls, vec!["giver", "other", "giver", "other"]);
    }

    #[test]
    fn a_turn_does_not_wait_for_a_poll_under_way_on_another_thread_which_polls_once_more() {
        let turns = Turns::default();
        let (entered, release) = (Arc::new(std::sync::Barrier::new(2)), Arc::new(std::sync::Barrier::new(2)));
        let polls = Arc::new(AtomicUsize::new(0));
        let mut first = true;
        let (e, r, p) = (Arc::clone(&entered), Arc::clone(&release), Arc::clone(&polls));
        let mut slow = Box::pin(turns.track(std::future::poll_fn(move |_| {
            p.fetch_add(1, Ordering::SeqCst);
            if std::mem::take(&mut first) {
                e.wait();
                r.wait();
            }
            Poll::<()>::Pending
        })));
        let poller = std::thread::spawn(move || {
            let waker = Waker::noop();
            assert!(slow.as_mut().poll(&mut Context::from_waker(waker)).is_pending());
        });
        entered.wait();
        let (gave, given) = std::sync::mpsc::channel();
        let giver = turns.clone();
        std::thread::spawn(move || {
            giver.give();
            let _ = gave.send(());
        });
        given.recv_timeout(std::time::Duration::from_secs(5)).expect("the turn waited for the poll under way");
        release.wait();
        poller.join().expect("the poller");
        assert_eq!(polls.load(Ordering::SeqCst), 2, "the poll under way polled once more for the turn");
    }

    #[test]
    fn a_dropped_task_gets_no_turn_and_a_panic_in_a_turn_is_resumed_by_its_task() {
        let turns = Turns::default();
        let polls = Arc::new(Mutex::new(Vec::new()));
        let dropped = turns.track(work("gone", Arc::new(Mutex::new(false)), Arc::clone(&polls)));
        drop(dropped);
        let mut panicking = Box::pin(turns.track(std::future::poll_fn(|_| -> Poll<()> { panic!("in the turn") })));
        turns.give();
        assert!(polls.lock().unwrap().is_empty(), "a dropped task's future was polled");
        let waker = Waker::noop();
        let resumed = catch_unwind(AssertUnwindSafe(|| panicking.as_mut().poll(&mut Context::from_waker(waker))));
        let payload = resumed.expect_err("the task's poll resumes the turn's panic");
        assert_eq!(payload.downcast_ref::<&str>(), Some(&"in the turn"));
    }
}
