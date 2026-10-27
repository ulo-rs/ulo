//! The harness pieces the conformance suites share (integrations DESIGN §9, I11): the runtime half
//! a suite's scenarios run on, the budget a scenario waits within, startup failures written to
//! files, the slots that bound how many scenarios hold an environment at once, and the TCP relay.
//! `ulo-rpc-conformance` and `ulo-db-conformance` depend on this crate; the first re-exports every
//! item at the path it had before the move.
//!
//! The crate names no runtime at default features. A suite's [`Harness`] is its runtime half:
//! the app's runtime, which every server and client in a scenario runs on and every wait and task
//! of the scenario's own goes through, and a `block_on` each scenario runs inside, beside a
//! one-thread `block_on` for the scenario that shows a connect does not stall its thread. `OnTokio`,
//! behind the `tokio` feature, is the harness of every suite on tokio; `OnSmol`, behind `smol`,
//! runs the scenarios on an `async-executor` `Executor`. The [`relay`] runs on threads of its own,
//! on `std` sockets, so it carries a client on either.
//!
//! A host short of memory bounds how many scenarios hold an environment at once by setting
//! [`PARALLEL_VAR`], `ULO_CONFORMANCE_PARALLEL`, to a positive integer: the bound is the smaller of
//! it and the suite's own declared bound, or it alone when the suite declares none. Unset or empty,
//! it bounds nothing, and libtest's `--test-threads` still applies on top of either.
//!
//! ```text
//! ULO_CONFORMANCE_PARALLEL=6 cargo test -p ulo-rpc-rabbitmq --features integration --test conformance
//! ```

use std::error::Error;
use std::future::Future;
use std::time::Duration;

use ulo::Runtime;

mod failures;
pub mod relay;
mod scripted;
#[cfg(feature = "smol")]
mod smol_harness;
#[cfg(feature = "smol")]
pub mod starvation;
#[cfg(feature = "tokio")]
mod tokio_harness;

pub use failures::failures_dir;
pub use scripted::ScriptedTimer;
#[cfg(feature = "smol")]
pub use smol_harness::OnSmol;
#[cfg(feature = "tokio")]
pub use tokio_harness::OnTokio;

/// The environment variable through which a host bounds how many scenarios of one suite hold an
/// environment at once. A value that is not a positive integer fails every scenario through
/// [`startup_failed!`].
pub const PARALLEL_VAR: &str = "ULO_CONFORMANCE_PARALLEL";

/// `error` and every source under it, joined by `: `, for a failure message: a scenario that fails
/// while its environment starts says what failed, the broker's container, the server's bind or the
/// client's connect, and why. A source whose text the message already carries is not repeated.
pub fn report(error: &(dyn Error + 'static)) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let line = cause.to_string();
        if !text.contains(&line) {
            text.push_str(": ");
            text.push_str(&line);
        }
        source = cause.source();
    }
    text
}

/// Fails a scenario whose environment did not start, as `panic!` with the same arguments would,
/// after writing the message to a file of its own under [`failures_dir`]; the panic names the file.
/// Every startup path in a suite fails through it, and a suite's environment implementation's own
/// startup should too.
#[macro_export]
macro_rules! startup_failed {
    ($($arg:tt)+) => {
        $crate::__private::startup_failed(::std::format!($($arg)+))
    };
}

/// The runtime a suite's scenarios run on, as the suite drives it: the app's runtime and the
/// executor each scenario is run to its end on.
pub trait Harness: Send + Sync + 'static {
    /// The app's runtime: every server and client a scenario starts runs on it, and the scenario's
    /// own sleeps, timeouts and tasks go through it.
    type Runtime: Runtime;

    /// A runtime value, taken wherever a scenario needs one: on the thread `block_on` runs and on
    /// every thread running the tasks the runtime spawns while it does.
    fn runtime() -> Self::Runtime;

    /// Runs `fut` to its end from a plain `#[test]` thread, on an executor that runs the tasks the
    /// runtime spawns while `fut` waits. Each scenario calls it once.
    fn block_on<F: Future>(fut: F) -> F::Output;

    /// [`block_on`](Self::block_on) on the calling thread alone: `fut` and every task the runtime
    /// spawns while it waits are polled there and nowhere else, so work that blocks the thread
    /// polling it stalls every one of them. The scenario that shows a link's connect and bind do
    /// not stall that thread runs through it, and fails on a harness that polls a task elsewhere.
    fn block_on_one_thread<F: Future>(fut: F) -> F::Output;
}

/// How long a scenario waits on its environment.
#[non_exhaustive]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// From `start` until the server answers its first call.
    pub boot: Duration,
    /// For a one-way effect to show: an event handled, a subscription in place.
    pub settle: Duration,
    /// From `disrupt` until calls succeed again.
    pub recovery: Duration,
    /// The longest the scenario's thread may go without polling a task that is due, while a link
    /// binds and connects on a harness's one thread: 50 ms unless [`with_stall`](Self::with_stall)
    /// sets another. [`new`](Self::new) leaves it at 50 ms whatever the other three are: a broker
    /// slow to answer is waited on, not blocked on, so it gives the link no reason to stall the
    /// thread, and a librdkafka handle's drop, about 100 ms, has to fail it on the slowest broker.
    pub stall: Duration,
}

/// [`Budget::stall`] unless a broker sets another.
const STALL: Duration = Duration::from_millis(50);

impl Budget {
    pub const fn new(boot: Duration, settle: Duration, recovery: Duration) -> Self {
        Budget { boot, settle, recovery, stall: STALL }
    }

    /// The budget with its [`stall`](Self::stall) set, for a host whose thread is descheduled for
    /// longer than 50 ms while it is busy.
    pub const fn with_stall(self, stall: Duration) -> Self {
        Budget { stall, ..self }
    }
}

impl Default for Budget {
    fn default() -> Self {
        Budget::new(Duration::from_secs(5), Duration::from_millis(500), Duration::from_secs(10))
    }
}

#[doc(hidden)]
pub mod __private {
    use std::num::NonZeroUsize;

    pub use crate::failures::startup_failed;
    use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

    use crate::PARALLEL_VAR;

    /// The slots one stamped suite's scenarios share. Each scenario runs on its own test thread,
    /// so a slot is waited for by blocking that thread before the scenario's runtime starts.
    pub struct Slots {
        held: Mutex<usize>,
        freed: Condvar,
    }

    impl Slots {
        pub const fn new() -> Self {
            Slots { held: Mutex::new(0), freed: Condvar::new() }
        }

        /// A slot when the suite's `declared` bound or the host's [`PARALLEL_VAR`] bounds the
        /// suite, the smaller when both do, held until the guard drops; `None` otherwise.
        pub fn hold(&'static self, declared: Option<NonZeroUsize>) -> Option<Slot> {
            let parallel = bound(declared, host_parallel())?;
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            while *held >= parallel.get() {
                held = self.freed.wait(held).unwrap_or_else(PoisonError::into_inner);
            }
            *held += 1;
            Some(Slot(self))
        }
    }

    impl Default for Slots {
        fn default() -> Self {
            Slots::new()
        }
    }

    /// One held slot, given back on drop, a scenario's panic included.
    pub struct Slot(&'static Slots);

    impl Drop for Slot {
        fn drop(&mut self) {
            *self.0.held.lock().unwrap_or_else(PoisonError::into_inner) -= 1;
            self.0.freed.notify_one();
        }
    }

    fn bound(declared: Option<NonZeroUsize>, host: Option<NonZeroUsize>) -> Option<NonZeroUsize> {
        match (declared, host) {
            (Some(declared), Some(host)) => Some(declared.min(host)),
            (declared, host) => declared.or(host),
        }
    }

    /// [`PARALLEL_VAR`] as the host set it, read once per test binary: `None` unset or empty.
    fn host_parallel() -> Option<NonZeroUsize> {
        static HOST: OnceLock<Option<NonZeroUsize>> = OnceLock::new();
        *HOST.get_or_init(|| match std::env::var(PARALLEL_VAR) {
            Err(std::env::VarError::NotPresent) => None,
            Ok(value) if value.trim().is_empty() => None,
            Ok(value) => match value.trim().parse::<NonZeroUsize>() {
                Ok(parallel) => Some(parallel),
                Err(_) => startup_failed(format!("{PARALLEL_VAR} is `{value}`, not a positive integer")),
            },
            Err(std::env::VarError::NotUnicode(value)) => startup_failed(format!("{PARALLEL_VAR} is {value:?}, not a positive integer")),
        })
    }

    #[cfg(test)]
    mod tests {
        use std::num::NonZeroUsize;

        use super::bound;

        fn n(value: usize) -> Option<NonZeroUsize> {
            NonZeroUsize::new(value)
        }

        #[test]
        fn the_smaller_of_two_bounds_applies() {
            assert_eq!(bound(n(4), n(6)), n(4), "a broker's bound below the host's");
            assert_eq!(bound(n(6), n(4)), n(4), "a host's bound below the broker's");
        }

        #[test]
        fn either_bound_alone_applies() {
            assert_eq!(bound(n(4), None), n(4), "the broker's bound, the host setting none");
            assert_eq!(bound(None, n(6)), n(6), "the host's bound, the broker declaring none");
            assert_eq!(bound(None, None), None, "neither bound");
        }
    }
}
