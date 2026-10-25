//! The one runner for user code the core waits on: a hook, a construction, a readiness attempt.
//! It applies the item's resolved bound and the shutdown cap through the app's `Timer`, and
//! catches panics at the poll boundary (§3.9, §10.2).
//!
//! Expiry drops the future: the item stops at its current await with its own state as it was,
//! and anything it spawned keeps running. Under `panic = "abort"` nothing can be caught.

use std::any::Any;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use crate::error::Limit;
use crate::redact::{Redacted, SecretRegistry, redact_panic};
use crate::timer::{Resolved, Timer, timeout};

/// The `shutdown_timeout` cap, counted from the trigger. A hook still running when it expires
/// reports `TimedOut { limit: ShutdownCap, after }`, whatever its own bound was.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cap {
    pub(crate) at: Instant,
    pub(crate) after: Duration,
}

impl Cap {
    /// Whether the cap has run out on `timer`'s clock.
    pub(crate) fn expired(&self, timer: &dyn Timer) -> bool {
        timer.now() >= self.at
    }
}

pub(crate) enum Outcome<T> {
    Done(T),
    Panicked(Redacted),
    TimedOut { after: Duration, limit: Limit },
}

/// Runs `fut` under `bound` and, at shutdown, `cap`; whichever limit fires first is the one
/// reported. Without a `Timer` only `Resolved::Unbounded` reaches here, the environment check
/// having refused every explicit bound.
///
/// The cap's `timeout` wraps the item and the item's own bound wraps that, and each `timeout`
/// polls what it wraps before its sleep: the item is polled before either limit, so an item that
/// completes on the poll where a limit fires counts as done, and the cap before the item's own
/// bound, so a tie reports `ShutdownCap`.
pub(crate) async fn run<F: Future>(
    timer: Option<&dyn Timer>,
    bound: Resolved,
    cap: Option<Cap>,
    secrets: &SecretRegistry,
    fut: F,
) -> Outcome<F::Output> {
    let item = CatchUnwind::new(fut);
    let capped = async {
        match (timer, cap) {
            (Some(t), Some(c)) => {
                timeout(t, c.at.saturating_duration_since(t.now()), item).await.map_err(|()| (c.after, Limit::ShutdownCap))
            }
            _ => Ok(item.await),
        }
    };
    let ended = match (timer, bound) {
        (Some(t), Resolved::After { after, limit }) => timeout(t, after, capped).await.unwrap_or(Err((after, limit))),
        _ => capped.await,
    };

    match ended {
        Ok(Ok(value)) => Outcome::Done(value),
        Ok(Err(payload)) => Outcome::Panicked(redact_panic(secrets, payload)),
        Err((after, limit)) => Outcome::TimedOut { after, limit },
    }
}

/// Polls the inner future inside `catch_unwind`, answering the payload on a panic. A future that
/// panicked must not be polled again; every caller drops it on the payload.
///
/// The inner future is boxed so the wrapper is `Unpin` and needs no pin projection.
pub(crate) struct CatchUnwind<F: ?Sized> {
    inner: Pin<Box<F>>,
}

impl<F> CatchUnwind<F> {
    pub(crate) fn new(inner: F) -> Self {
        CatchUnwind { inner: Box::pin(inner) }
    }
}

impl<F: ?Sized> CatchUnwind<F> {
    /// Over a future already boxed, a spawned `BoxFuture`, without boxing it again.
    pub(crate) fn boxed(inner: Pin<Box<F>>) -> Self {
        CatchUnwind { inner }
    }
}

impl<F: Future + ?Sized> Future for CatchUnwind<F> {
    type Output = Result<F::Output, Box<dyn Any + Send>>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        // A future that panicked is never polled again, so no broken invariant inside it is
        // observed afterwards; only its drop runs.
        match catch_unwind(AssertUnwindSafe(|| this.inner.as_mut().poll(cx))) {
            Ok(Poll::Ready(value)) => Poll::Ready(Ok(value)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::poll_fn;
    use std::pin::pin;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::Waker;

    use super::*;
    use crate::timer::BoxFuture;

    /// A clock that moves only when the test advances it; a sleep is ready once the clock has
    /// reached the instant it was asked to end at.
    #[derive(Clone)]
    struct Scripted(Arc<Mutex<Instant>>);

    impl Scripted {
        fn new() -> Scripted {
            Scripted(Arc::new(Mutex::new(Instant::now())))
        }

        fn advance(&self, by: Duration) {
            *self.0.lock().unwrap() += by;
        }
    }

    impl Timer for Scripted {
        fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
            let clock = Arc::clone(&self.0);
            let until = *clock.lock().unwrap() + d;
            Box::pin(poll_fn(move |_| if *clock.lock().unwrap() >= until { Poll::Ready(()) } else { Poll::Pending }))
        }

        fn now(&self) -> Instant {
            *self.0.lock().unwrap()
        }
    }

    const OWN: Duration = Duration::from_secs(2);
    /// The cap as configured; counted from a trigger that is not now, so it differs from what is
    /// left of it.
    const CAP: Duration = Duration::from_secs(9);

    fn poll<F: Future>(fut: Pin<&mut F>) -> Poll<F::Output> {
        fut.poll(&mut Context::from_waker(Waker::noop()))
    }

    /// The item's own bound and what is left of the cap end at the same instant.
    fn coinciding(clock: &Scripted) -> (Resolved, Option<Cap>) {
        (Resolved::After { after: OWN, limit: Limit::Item }, Some(Cap { at: clock.now() + OWN, after: CAP }))
    }

    #[test]
    fn a_cap_and_an_own_bound_ending_on_one_poll_report_the_cap() {
        let clock = Scripted::new();
        let (bound, cap) = coinciding(&clock);
        let secrets = SecretRegistry::default();
        let mut running = pin!(run(Some(&clock), bound, cap, &secrets, std::future::pending::<()>()));
        assert!(poll(running.as_mut()).is_pending(), "a limit fired before the clock moved");
        clock.advance(OWN);
        match poll(running.as_mut()) {
            Poll::Ready(Outcome::TimedOut { after, limit }) => {
                assert_eq!((after, limit), (CAP, Limit::ShutdownCap), "the cap, wrapped inside the item's own bound, is polled first");
            }
            Poll::Ready(Outcome::Done(())) => panic!("a pending item was reported done"),
            Poll::Ready(Outcome::Panicked(_)) => panic!("a pending item was reported panicked"),
            Poll::Pending => panic!("neither limit fired once both had passed"),
        }
    }

    #[test]
    fn an_item_ready_on_the_poll_both_limits_end_counts_as_done() {
        let clock = Scripted::new();
        let (bound, cap) = coinciding(&clock);
        let secrets = SecretRegistry::default();
        let finished = AtomicBool::new(false);
        let item = poll_fn(|_| if finished.load(Ordering::SeqCst) { Poll::Ready(7) } else { Poll::Pending });
        let mut running = pin!(run(Some(&clock), bound, cap, &secrets, item));
        assert!(poll(running.as_mut()).is_pending(), "the item finished before it was let");
        finished.store(true, Ordering::SeqCst);
        clock.advance(OWN);
        match poll(running.as_mut()) {
            Poll::Ready(Outcome::Done(value)) => assert_eq!(value, 7),
            Poll::Ready(Outcome::TimedOut { after, limit }) => panic!("a limit polled before the item: {limit:?} after {after:?}"),
            Poll::Ready(Outcome::Panicked(_)) => panic!("the item was reported panicked"),
            Poll::Pending => panic!("the item was ready and not taken"),
        }
    }
}
