//! A value whose changes wake the tasks waiting on it, without a runtime: a `std` mutex for the
//! value and an `event-listener` event for the wake-up. `ulo-rpc`'s server keeps its close signal
//! and drain state in one, `ulo-ws`'s connection tracker its phase, drain token and count of live
//! connections, `ulo-server-hyper`'s accept loop its drain and close signals, and the TCP links'
//! wire its phase and the calls it holds.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll};

use event_listener::{Event, EventListener};

/// A value and the event its changes notify.
///
/// For state a link or host changes from synchronous code and waits on under a predicate: a close
/// signal, a drain's progress, a count of live connections.
///
/// An `Event` keeps no permit for a waiter that has not registered yet, so every wait registers
/// its listener before it reads the value: a change made between the read and the wait then still
/// wakes it.
pub struct Watch<T> {
    value: Mutex<T>,
    changed: Event,
}

impl<T> Watch<T> {
    pub fn new(value: T) -> Self {
        Watch { value: Mutex::new(value), changed: Event::new() }
    }

    /// Changes the value under its lock, then wakes every waiter.
    pub fn modify(&self, change: impl FnOnce(&mut T)) {
        change(&mut self.lock());
        self.changed.notify(usize::MAX);
    }

    pub fn read<R>(&self, read: impl FnOnce(&T) -> R) -> R {
        read(&self.lock())
    }

    /// A future that resolves at the next change. Taken before the value is read, it is woken by
    /// any change made after the read, which is what a hand-written poll loop needs where
    /// [`wait_for`](Self::wait_for) does not fit.
    pub fn changed(&self) -> Changed {
        Changed(self.changed.listen())
    }

    /// Resolves once `ready` holds for the value.
    pub async fn wait_for(&self, ready: impl Fn(&T) -> bool) {
        loop {
            let changed = self.changed.listen();
            if self.read(&ready) {
                return;
            }
            changed.await;
        }
    }

    fn lock(&self) -> MutexGuard<'_, T> {
        self.value.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The next change of a [`Watch`], from [`Watch::changed`]. Resolves once, at the first change
/// after it was taken.
#[derive(Debug)]
pub struct Changed(EventListener);

impl Future for Changed {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Waker};

    use super::Watch;

    /// A writer's change landing between `wait_for`'s read of the value and its wait: the read sees
    /// the old value, and the change and its notification follow at once. The writer is modelled
    /// inside `ready`, after it has read, since the value's lock is held there; the notification
    /// goes to the event directly for the same reason.
    #[test]
    fn a_change_between_the_read_and_the_wait_wakes_the_waiter() {
        let watch = Watch::new(AtomicBool::new(false));
        let first = AtomicBool::new(true);
        let waiting = watch.wait_for(|value| {
            let seen = value.load(Ordering::SeqCst);
            if first.swap(false, Ordering::SeqCst) {
                value.store(true, Ordering::SeqCst);
                watch.changed.notify(usize::MAX);
            }
            seen
        });
        let mut waiting = pin!(waiting);
        let polled = waiting.as_mut().poll(&mut Context::from_waker(Waker::noop()));
        assert!(
            polled.is_ready(),
            "a change made after the value was read and before the wait began was missed: the wait is still pending with the value \
             now {}",
            watch.read(|value| value.load(Ordering::SeqCst))
        );
    }

    /// `Changed` registers when it is taken, not when it is first polled: one taken before a change
    /// and polled only after it resolves, and one taken after the change waits for the next.
    #[test]
    fn a_changed_taken_before_a_change_resolves_though_first_polled_after_it() {
        let watch = Watch::new(0u32);
        let mut cx = Context::from_waker(Waker::noop());
        let mut unchanged = pin!(watch.changed());
        assert!(unchanged.as_mut().poll(&mut cx).is_pending(), "a `Changed` resolved with no change made");
        let mut taken = pin!(watch.changed());
        watch.modify(|value| *value += 1);
        assert!(
            taken.as_mut().poll(&mut cx).is_ready(),
            "a `Changed` taken before a change and first polled after it missed the change"
        );
        let mut later = pin!(watch.changed());
        assert!(later.as_mut().poll(&mut cx).is_pending(), "a `Changed` taken after the change resolved for it");
    }
}
