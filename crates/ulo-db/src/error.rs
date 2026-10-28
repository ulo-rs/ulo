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

/// The migration step of a store module failed, and whether retrying the start can mend it.
///
/// - [`waited_for_lock`](Self::waited_for_lock): the wait for the migration lock reached its bound
///   (`connect_within`). Another replica is migrating, and a restart after it finishes succeeds.
/// - [`is_retryable`](Self::is_retryable) without that: another failure of the lock step, a lost
///   connection or a server restarted between the ping and the lock. The store's own classifier
///   decides, the one its `ping` uses: an error it marks `ulo::Permanent`, such as no permission to
///   take the lock, is not retryable and any other is.
/// - Not retryable: a migration that failed, the migrations running past
///   `migrate_within` once the lock was held, a task that panicked, or either bound refused by a
///   runtime that has shut down, whose `source` is a `ulo::RuntimeShutDown`.
///
/// The module's init hook raises a retryable failure without `ulo::Permanent`, so
/// `ulo::is_permanent` answers false for it, and every other inside `ulo::Permanent`.
#[non_exhaustive]
#[derive(Debug)]
pub struct MigrationFailed {
    pub module: &'static str,
    pub source: BoxError,
    retry: Retry,
}

/// What retrying the start can do for a [`MigrationFailed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retry {
    No,
    /// The lock wait reached its bound.
    LockWait,
    /// The lock step failed in a way the store's classifier does not call permanent.
    LockStep,
}

impl MigrationFailed {
    pub(crate) fn new(module: &'static str, source: BoxError, retry: Retry) -> Self {
        MigrationFailed { module, source, retry }
    }

    /// Whether the failure is the wait for the migration lock reaching its bound.
    pub fn waited_for_lock(&self) -> bool {
        self.retry == Retry::LockWait
    }

    /// Whether retrying this start, with this configuration on this runtime, can succeed: the lock
    /// wait ran out, or the lock step failed in a way the store does not call permanent.
    pub fn is_retryable(&self) -> bool {
        self.retry != Retry::No
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

/// `.migrate_within(Bound::After(Duration::ZERO))`, refused at registration through `try_value`,
/// since a zero bound would fail every migration.
#[derive(Debug)]
pub(crate) struct ZeroMigrateBound {
    store: &'static str,
}

impl ZeroMigrateBound {
    pub(crate) fn new(store: &'static str) -> Self {
        ZeroMigrateBound { store }
    }
}

impl fmt::Display for ZeroMigrateBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the {} store module's `migrate_within` is zero; the migration bound must be positive", self.store)
    }
}

impl Error for ZeroMigrateBound {}

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

/// The migrations passed `migrate_within`, counted from the moment the lock was held.
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
