use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64};

use crate::run::{Report, Runner};

/// The app's health as the probes read it, bound by [`HealthModule`](crate::HealthModule) and
/// injected as `Dep<Readiness>`.
///
/// Liveness is true whenever the process answers, plus any check declared `Probe::Liveness`.
/// Readiness is true while the app is `Running` and every readiness check is up, and false from
/// `Stopping`. Startup is true once `listen` has finished, which holds whenever a probe is answered.
#[derive(Clone)]
pub struct Readiness {
    pub(crate) shared: Arc<State>,
}

pub(crate) struct State {
    pub(crate) runner: Runner,
    pub(crate) stopping: AtomicBool,
    /// Bumped when `stopping` flips, which [`Changes`] waits on.
    pub(crate) generation: AtomicU64,
}

impl Readiness {
    pub(crate) fn new(runner: Runner) -> Self {
        Readiness {
            shared: Arc::new(State { runner, stopping: AtomicBool::new(false), generation: AtomicU64::new(0) }),
        }
    }

    pub async fn liveness(&self) -> Arc<Report> {
        todo!()
    }

    /// Runs the readiness checks, or answers a report that is down without running them once the
    /// app has reached `Stopping`.
    pub async fn readiness(&self) -> Arc<Report> {
        todo!()
    }

    pub async fn startup(&self) -> Arc<Report> {
        todo!()
    }

    /// Whether the app has reached `Stopping`, as the before-shutdown hook recorded.
    pub fn is_stopping(&self) -> bool {
        todo!()
    }

    /// A notice that readiness may have changed, for the gRPC bridge: it resolves when the app
    /// reaches `Stopping`.
    pub fn subscribe(&self) -> Changes {
        Changes { shared: Arc::clone(&self.shared), seen: 0 }
    }
}

/// Resolves each time the readiness state changes after the last notice it delivered.
pub struct Changes {
    pub(crate) shared: Arc<State>,
    pub(crate) seen: u64,
}

impl Changes {
    pub async fn changed(&mut self) {
        todo!()
    }
}
