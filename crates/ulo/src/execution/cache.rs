use std::collections::HashMap;
use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

use async_lock::OnceCell;

use crate::binding::Instance;
use crate::error::LookupError;
use crate::graph::BindingId;
use crate::key::KeyName;
use crate::redact::Redactor;
use crate::transport::enhancer::ClosureId;

/// What one cell holds the instance of: a binding, or an enhancer declared by closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Slot {
    Binding(BindingId),
    Closure(ClosureId),
}

/// One build per slot, run on first read and shared after. Readers of one slot concurrently await
/// one cell, and the build runs once for all of them.
///
/// An execution's cache keeps a failed build's error as it keeps an instance (§3.8): every reader
/// of the slot in that execution, concurrent or later, receives that error, and the build does
/// not run again until another execution reads the slot. The app's cache, which holds the
/// enhancer closures built once, keeps no failure: a later call builds again, since a failure
/// kept there would last as long as the app.
///
/// A cell holds the instance as the recipe built it; `AppShared::obtain` widens a contribution to
/// its collection's type on the way out, as it does for a stored singleton.
pub(crate) struct OnceCells {
    /// `None` once released: the execution has ended.
    cells: Mutex<Option<HashMap<Slot, Arc<OnceCell<Built>>>>>,
    keeps_failures: bool,
}

/// What a cell holds: the instance, or the failure prepared for sharing.
type Built = Result<Instance, LookupError>;

impl OnceCells {
    /// An execution's cache, which keeps a failed build for the execution.
    pub(crate) fn per_execution() -> Self {
        OnceCells { cells: Mutex::new(Some(HashMap::new())), keeps_failures: true }
    }

    /// The app's cache of enhancer closures built once, which keeps no failure.
    pub(crate) fn per_app() -> Self {
        OnceCells { cells: Mutex::new(Some(HashMap::new())), keeps_failures: false }
    }

    /// The instance in `slot`, built by `build` on the first read. Once the cache is released it
    /// answers `ExecutionEnded` naming `key` and builds nothing. A build already running when the
    /// cache is released completes for the readers awaiting it, and its outcome is not kept.
    pub(crate) async fn get_or_build<F, Fut>(&self, slot: Slot, key: KeyName, build: F) -> Result<Instance, LookupError>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Instance, LookupError>>,
    {
        // The map lock is released before the build is awaited: the build reads other dependencies,
        // which come back here for their own cells.
        let cell = {
            let mut cells = self.cells.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(cells) = cells.as_mut() else {
                return Err(LookupError::ExecutionEnded { key });
            };
            Arc::clone(cells.entry(slot).or_default())
        };
        if !self.keeps_failures {
            let instance = cell.get_or_try_init(|| async { build().await.map(Ok) }).await?;
            return instance.as_ref().map(Arc::clone).map_err(LookupError::share);
        }
        let built = cell.get_or_init(|| async { build().await.map_err(LookupError::into_shared) }).await;
        built.as_ref().map(Arc::clone).map_err(LookupError::share)
    }

    /// Drops every instance the cache holds and refuses every later read. The instances drop
    /// outside the lock: an instance's drop may read the cache, which would deadlock under it.
    ///
    /// Each instance drops under `catch_unwind`: a panicking `Drop` does not stop the rest from
    /// dropping or the execution from ending. The first panic is logged at `warn`, redacted.
    pub(crate) fn release(&self, redactor: &Redactor) {
        let released = self.cells.lock().unwrap_or_else(PoisonError::into_inner).take();
        let mut first = None;
        for (_, cell) in released.into_iter().flatten() {
            if let Err(payload) = catch_unwind(AssertUnwindSafe(move || drop(cell))) {
                first.get_or_insert(payload);
            }
        }
        if let Some(payload) = first {
            let message = redactor.panic(payload);
            tracing::warn!(%message, "an execution-scoped instance panicked while it was dropped at the end of its execution");
        }
    }
}
