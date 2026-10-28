use std::error::Error;
use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::task::Poll;
use std::time::{Duration, Instant};

use crate::error::Limit;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub type BoxError = Box<dyn Error + Send + Sync + 'static>;

/// The app's one clock. Every wait the core times, the drain, a hook, a construction, a
/// readiness check, runs through it; it is set on the builder and supplied by a runtime adapter.
///
/// `sleep` returns a boxed `'static` future so the trait stays dyn-compatible and a caller can
/// spawn what it returns. `now` sits on the same trait because deadlines and sleeps have to read
/// one clock: measured with `std::time::Instant`, a deadline on a paused test clock never
/// arrives while sleeps keep resolving. When a `Timer` is configured, services read it as
/// `Dep<dyn Timer>`.
///
/// A clock whose runtime has shut down refuses to time, as a value and never a panic:
/// [`try_sleep`](Self::try_sleep) answers `Err(Refused)`, and `sleep` ends at once. `now` still
/// answers.
pub trait Timer: Send + Sync + 'static {
    /// Ends once `d` has passed, or at once on a clock whose runtime has shut down, before the
    /// sleep or during it. [`try_sleep`](Self::try_sleep) tells the two ends apart.
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()>;

    fn now(&self) -> Instant;

    /// `sleep`, answering `Err(Refused)` when the clock's runtime has shut down, before the sleep
    /// or during it. Code that would otherwise act on a deadline it can no longer keep reads this.
    /// A clock that cannot shut down answers `sleep`'s end as `Ok`. A wrapper forwards this to the
    /// clock it wraps, as it forwards `sleep`: answering `Ok` there drops that clock's refusal.
    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>>;
}

/// What a runtime that has shut down answers [`Timer::try_sleep`] and
/// [`Spawn::try_spawn`](crate::Spawn::try_spawn) with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the runtime has shut down")
    }
}

impl Error for Refused {}

/// The reason a failure the runtime's refusal caused is reported under: by [`RuntimeShutDown`],
/// and by a transport that answers it as a wire reason, as an RPC call's `ErrorInfo` does.
pub const RUNTIME_SHUT_DOWN: &str = "runtime_shut_down";

/// Work that ended because the runtime refused a wait, having shut down: a loop whose pacing wait
/// was refused, or a bounded wait whose bound was. Reported where a reader learns why the work
/// ended, in place of an elapsed bound or a loop out of attempts, neither of which happened.
/// Retrying on a runtime that has shut down cannot succeed, so whoever raises one in a startup
/// error raises it inside [`Permanent`](crate::Permanent).
///
/// `source()` is the last attempt's error for a loop ended [`after`](Self::after) one, and the
/// runtime's [`Refused`] otherwise.
#[derive(Debug)]
pub struct RuntimeShutDown {
    source: BoxError,
}

impl RuntimeShutDown {
    pub fn new(refused: Refused) -> RuntimeShutDown {
        RuntimeShutDown { source: Box::new(refused) }
    }

    /// A loop ended on the refusal after an attempt that failed with `last`.
    pub fn after(last: impl Into<BoxError>) -> RuntimeShutDown {
        RuntimeShutDown { source: last.into() }
    }

    /// [`RUNTIME_SHUT_DOWN`].
    pub fn reason(&self) -> &'static str {
        RUNTIME_SHUT_DOWN
    }
}

impl std::fmt::Display for RuntimeShutDown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the runtime has shut down")
    }
}

impl Error for RuntimeShutDown {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&*self.source)
    }
}

/// How long a hook, a construction or a readiness check may take.
///
/// `Default` takes the app's default for that kind of item: `hook_timeout` (10 s) for a hook,
/// `construct_timeout` (30 s) for a construction and for one attempt of a readiness check. The
/// defaults apply only when a `Timer` is configured; without one an item left at `Default` runs
/// unbounded. `After(d)` needs a `Timer` and is a wiring error without one. `Unbounded` needs
/// none and is still contained by `shutdown_timeout` at shutdown.
///
/// `Unbounded` is a variant rather than `After(Duration::MAX)` because `Instant + Duration::MAX`
/// panics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Bound {
    #[default]
    Default,
    After(Duration),
    Unbounded,
}

/// The kind of item a bound applies to, which picks the app default `Bound::Default` takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BoundKind {
    Hook,
    Construction,
    /// One attempt of a readiness check.
    ReadinessAttempt,
    /// A readiness check as a whole, retries and backoff included. Its `Default` is no bound:
    /// only the attempt takes `construct_timeout`.
    ReadinessWhole,
}

/// A bound after the app defaults and the presence of a `Timer` are applied.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolved {
    /// Expiry reports `TimedOut { after, limit }`.
    After { after: Duration, limit: Limit },
    Unbounded,
}

/// The app defaults a `Bound::Default` resolves against.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Defaults {
    pub(crate) hook_timeout: Duration,
    pub(crate) construct_timeout: Duration,
}

/// Applies the §3.9 rule: an explicit bound is `Limit::Item` (or `Limit::Attempt` for a
/// readiness attempt), `Default` is the app default only when a `Timer` exists, `Unbounded`
/// is none. The environment pass has already refused an explicit bound with no `Timer`, and an
/// `After(ZERO)` on every item this resolves, as `WiringError::ZeroBound`.
pub(crate) fn resolve_bound(bound: Bound, kind: BoundKind, defaults: Option<&Defaults>) -> Resolved {
    // Without a `Timer` nothing can time the item, whatever it wrote.
    let Some(defaults) = defaults else {
        return Resolved::Unbounded;
    };
    match bound {
        Bound::Unbounded => Resolved::Unbounded,
        Bound::After(after) => {
            let limit = match kind {
                BoundKind::ReadinessAttempt => Limit::Attempt,
                BoundKind::Hook | BoundKind::Construction | BoundKind::ReadinessWhole => Limit::Item,
            };
            Resolved::After { after, limit }
        }
        Bound::Default => match kind {
            BoundKind::Hook => Resolved::After { after: defaults.hook_timeout, limit: Limit::Default },
            BoundKind::Construction | BoundKind::ReadinessAttempt => {
                Resolved::After { after: defaults.construct_timeout, limit: Limit::Default }
            }
            BoundKind::ReadinessWhole => Resolved::Unbounded,
        },
    }
}

/// `fut` raced against `timer.sleep(d)`: `Err(())` when the sleep wins, dropping `fut` at its
/// current await. `fut` is polled first: work that completes in the poll the sleep fires in
/// counts as done.
pub(crate) async fn timeout<F: Future>(timer: &dyn Timer, d: Duration, fut: F) -> Result<F::Output, ()> {
    let mut fut = pin!(fut);
    let mut sleep = timer.sleep(d);
    poll_fn(|cx| {
        if let Poll::Ready(output) = fut.as_mut().poll(cx) {
            return Poll::Ready(Ok(output));
        }
        match sleep.as_mut().poll(cx) {
            Poll::Ready(()) => Poll::Ready(Err(())),
            Poll::Pending => Poll::Pending,
        }
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::task::{Context, Waker};
    use std::time::Instant;

    use super::*;

    const HOOK: Duration = Duration::from_secs(10);
    const CONSTRUCT: Duration = Duration::from_secs(30);
    const SET: Duration = Duration::from_secs(3);

    const KINDS: [BoundKind; 4] =
        [BoundKind::Hook, BoundKind::Construction, BoundKind::ReadinessAttempt, BoundKind::ReadinessWhole];

    fn defaults() -> Defaults {
        Defaults { hook_timeout: HOOK, construct_timeout: CONSTRUCT }
    }

    #[test]
    fn a_bound_is_default_until_written() {
        assert_eq!(Bound::default(), Bound::Default);
    }

    #[test]
    fn without_a_timer_every_bound_runs_unbounded() {
        for kind in KINDS {
            for bound in [Bound::Default, Bound::After(SET), Bound::Unbounded] {
                assert_eq!(resolve_bound(bound, kind, None), Resolved::Unbounded, "{bound:?} on {kind:?}");
            }
        }
    }

    #[test]
    fn unbounded_stays_unbounded_with_a_timer() {
        for kind in KINDS {
            assert_eq!(resolve_bound(Bound::Unbounded, kind, Some(&defaults())), Resolved::Unbounded, "{kind:?}");
        }
    }

    #[test]
    fn an_explicit_bound_is_the_items_own_limit_and_a_readiness_attempts_is_the_attempt_limit() {
        let defaults = defaults();
        for kind in [BoundKind::Hook, BoundKind::Construction, BoundKind::ReadinessWhole] {
            assert_eq!(
                resolve_bound(Bound::After(SET), kind, Some(&defaults)),
                Resolved::After { after: SET, limit: Limit::Item },
                "{kind:?}"
            );
        }
        assert_eq!(
            resolve_bound(Bound::After(SET), BoundKind::ReadinessAttempt, Some(&defaults)),
            Resolved::After { after: SET, limit: Limit::Attempt }
        );
    }

    #[test]
    fn default_takes_the_app_default_for_the_kind_of_item() {
        let defaults = defaults();
        let default = |after| Resolved::After { after, limit: Limit::Default };
        assert_eq!(resolve_bound(Bound::Default, BoundKind::Hook, Some(&defaults)), default(HOOK));
        assert_eq!(resolve_bound(Bound::Default, BoundKind::Construction, Some(&defaults)), default(CONSTRUCT));
        assert_eq!(resolve_bound(Bound::Default, BoundKind::ReadinessAttempt, Some(&defaults)), default(CONSTRUCT));
        // Only an attempt of a readiness check takes `construct_timeout`; the check as a whole has no default.
        assert_eq!(resolve_bound(Bound::Default, BoundKind::ReadinessWhole, Some(&defaults)), Resolved::Unbounded);
    }

    /// A clock whose sleep is ready at once, or never, and which records the duration it was asked for.
    struct Clock {
        ready: bool,
        asked: std::sync::Mutex<Option<Duration>>,
    }

    impl Timer for Clock {
        fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
            *self.asked.lock().unwrap() = Some(d);
            if self.ready { Box::pin(std::future::ready(())) } else { Box::pin(std::future::pending()) }
        }

        fn now(&self) -> Instant {
            Instant::now()
        }

        fn try_sleep(&self, d: Duration) -> crate::BoxFuture<'static, Result<(), crate::Refused>> {
            let sleep = crate::Timer::sleep(self, d);
            Box::pin(async move {
                sleep.await;
                Ok(())
            })
        }
    }

    fn block_on<F: Future>(fut: F) -> F::Output {
        let mut fut = pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        // Every future here finishes within a few polls; a bound turns a hang into a failure.
        for _ in 0..100 {
            if let Poll::Ready(output) = fut.as_mut().poll(&mut cx) {
                return output;
            }
        }
        panic!("the future did not complete in 100 polls");
    }

    #[test]
    fn the_future_that_finishes_first_wins() {
        let clock = Clock { ready: false, asked: Default::default() };
        let out = block_on(timeout(&clock, SET, async { 7 }));
        assert_eq!(out, Ok(7));
        assert_eq!(*clock.asked.lock().unwrap(), Some(SET), "the sleep is asked for the bound");
    }

    #[test]
    fn the_sleep_wins_when_the_future_never_finishes() {
        let clock = Clock { ready: true, asked: Default::default() };
        let out = block_on(timeout(&clock, SET, std::future::pending::<u8>()));
        assert_eq!(out, Err(()));
    }

    #[test]
    fn work_that_completes_in_the_poll_the_sleep_fires_in_counts_as_done() {
        let clock = Clock { ready: true, asked: Default::default() };
        let out = block_on(timeout(&clock, SET, async { 7 }));
        assert_eq!(out, Ok(7));
    }
}
