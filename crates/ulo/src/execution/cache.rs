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

/// One instance per slot, built on first read and shared after. Two readers of one slot
/// concurrently get one instance: both await one cell. A failed build is not cached.
///
/// An execution's cache holds its execution-scoped bindings and its per-execution enhancer
/// closures; the app's holds the enhancer closures built once. A cell holds the instance as the
/// recipe built it; `AppShared::obtain` widens a contribution to its collection's type on the way
/// out, as it does for a stored singleton.
pub(crate) struct OnceCells {
    /// `None` once released: the execution has ended.
    cells: Mutex<Option<HashMap<Slot, Arc<OnceCell<Instance>>>>>,
}

impl Default for OnceCells {
    fn default() -> Self {
        OnceCells { cells: Mutex::new(Some(HashMap::new())) }
    }
}

impl OnceCells {
    /// The instance in `slot`, built by `build` on the first read. Once the cache is released it
    /// answers `ExecutionEnded` naming `key` and builds nothing. A build already running when the
    /// cache is released completes for the readers awaiting it, and its instance is not kept.
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
        let instance = cell.get_or_try_init(build).await?;
        Ok(Arc::clone(instance))
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
