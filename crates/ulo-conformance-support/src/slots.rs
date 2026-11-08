//! The bound on how many scenarios of one suite hold an environment at once.

use std::num::NonZeroUsize;
use std::sync::{Condvar, Mutex, OnceLock, PoisonError};

use crate::PARALLEL_VAR;
use crate::failures::startup_failed;

/// The slots one stamped suite's scenarios share. A suite's stamping macro declares one as a
/// `static` beside the tests it writes, and each test holds a slot from before its environment
/// starts until the scenario has returned; an environment or link author names it nowhere. Each
/// scenario runs on its own test thread, so a slot is waited for by blocking that thread before
/// the scenario's runtime starts.
pub struct Slots {
    held: Mutex<usize>,
    freed: Condvar,
}

impl Slots {
    /// No slot held, a `const fn` for the `static` a suite declares.
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
