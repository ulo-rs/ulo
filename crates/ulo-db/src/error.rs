use std::error::Error;
use std::fmt;
use std::time::Duration;

use ulo::{BoxError, TaskEnd};

/// The app's runtime is not one the store supports. Raised inside `ulo::Permanent` from the pool's
/// factory at `connect`, naming the module, the runtime it needs and the runtime it was given.
#[non_exhaustive]
#[derive(Debug)]
pub struct UnsupportedRuntime {
    pub module: &'static str,
    pub required: &'static str,
    /// The name `Spawn::name` answered; `"unknown"` for a runtime that declares none.
    pub given: &'static str,
}

impl UnsupportedRuntime {
    pub(crate) fn new(module: &'static str, required: &'static str, given: &'static str) -> Self {
        UnsupportedRuntime { module, required, given }
    }
}

impl fmt::Display for UnsupportedRuntime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} needs the {} runtime, and the app was given {}", self.module, self.required, self.given)
    }
}

impl Error for UnsupportedRuntime {}

/// The migration step of a store module failed, in one of two ways that restart differently.
///
/// - The wait for the migration lock reached the bound ([`waited_for_lock`](Self::waited_for_lock)
///   is true): another replica is migrating, and a restart after it finishes would succeed. The
///   module's init hook raises it without `ulo::Permanent`, so `ulo::is_permanent` answers false.
/// - Anything else: the lock could not be taken for another reason, a migration failed, or the
///   migrations themselves ran past the bound. The hook raises it inside `ulo::Permanent`, since a
///   restart of the same build and configuration meets the same failure.
#[non_exhaustive]
#[derive(Debug)]
pub struct MigrationFailed {
    pub module: &'static str,
    pub source: BoxError,
    lock_wait: bool,
}

impl MigrationFailed {
    pub(crate) fn new(module: &'static str, source: BoxError, lock_wait: bool) -> Self {
        MigrationFailed { module, source, lock_wait }
    }

    /// Whether the failure is the wait for the migration lock reaching the bound, the one
    /// retryable migration failure.
    pub fn waited_for_lock(&self) -> bool {
        self.lock_wait
    }
}

impl fmt::Display for MigrationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} migrations failed: {}", self.module, self.source)
    }
}

impl Error for MigrationFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// A task `ulo-db` spawned on the app's runtime ended without a value: its future panicked, or the
/// runtime dropped it unfinished. The panic's message was redacted inside the task.
#[derive(Debug)]
pub(crate) struct TaskFailed {
    store: &'static str,
    step: &'static str,
    end: TaskEnd,
}

impl TaskFailed {
    pub(crate) fn new(store: &'static str, step: &'static str, end: TaskEnd) -> Self {
        TaskFailed { store, step, end }
    }
}

impl fmt::Display for TaskFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.end {
            TaskEnd::Panicked(message) => write!(f, "the {} store's {} panicked: {message}", self.store, self.step),
            TaskEnd::Aborted => write!(f, "the {} store's {} was dropped before it finished", self.store, self.step),
            TaskEnd::Finished => write!(f, "the {} store's {} finished without a value", self.store, self.step),
        }
    }
}

impl Error for TaskFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match &self.end {
            TaskEnd::Panicked(message) => Some(&**message),
            _ => None,
        }
    }
}

/// `.connect_within(Duration::ZERO)`, refused at registration through `try_value`, since a zero
/// bound would fail every start.
#[derive(Debug)]
pub(crate) struct ZeroConnectBound {
    store: &'static str,
}

impl ZeroConnectBound {
    pub(crate) fn new(store: &'static str) -> Self {
        ZeroConnectBound { store }
    }
}

impl fmt::Display for ZeroConnectBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} store module's `connect_within` is zero; the connect bound must be positive", self.store)
    }
}

impl Error for ZeroConnectBound {}

/// The store's `MigrationLock::lock` answered an error, as `MigrationFailed`'s source.
#[derive(Debug)]
pub(crate) struct LockFailed(pub(crate) BoxError);

impl fmt::Display for LockFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "taking the migration lock: {}", self.0)
    }
}

impl Error for LockFailed {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.0.as_ref())
    }
}

/// The wait for the migration lock reached its bound, because another holder kept it. A store's
/// `MigrationLock::lock` returns this, boxed, when the wait it bounded on the server or on the
/// file ran out, so `ulo-db` can tell a held lock from a lock that could not be taken at all.
#[non_exhaustive]
#[derive(Debug)]
pub struct LockTimedOut {
    pub bound: Duration,
}

impl LockTimedOut {
    pub fn new(bound: Duration) -> Self {
        LockTimedOut { bound }
    }
}

impl fmt::Display for LockTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the migration lock was not obtained within {:?}", self.bound)
    }
}

impl Error for LockTimedOut {}

/// The migrations, or the release after them, passed the module's bound once the lock was held.
#[derive(Debug)]
pub(crate) struct MigrationTimedOut {
    pub(crate) bound: Duration,
}

impl fmt::Display for MigrationTimedOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the migrations did not finish within {:?}", self.bound)
    }
}

impl Error for MigrationTimedOut {}

/// `.lazy()` on a module whose options make `create` connect, refused at registration through
/// `try_value`.
#[derive(Debug)]
pub(crate) struct LazyRefused {
    store: &'static str,
    because: &'static str,
}

impl LazyRefused {
    pub(crate) fn new(store: &'static str, because: &'static str) -> Self {
        LazyRefused { store, because }
    }
}

impl fmt::Display for LazyRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} store module cannot be lazy: {}", self.store, self.because)
    }
}

impl Error for LazyRefused {}
