use std::sync::{Arc, Mutex};

use futures_util::future::Shared;
use ulo::{BoxFuture, Timer};

use crate::check::{Check, Health, Probe};

/// The outcome of one check in one run.
#[derive(Debug)]
pub struct CheckResult {
    pub(crate) name: String,
    pub(crate) probe: Probe,
    pub(crate) health: Health,
}

impl CheckResult {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn probe(&self) -> Probe {
        self.probe
    }

    pub fn health(&self) -> &Health {
        &self.health
    }
}

/// Every check of one run.
#[derive(Debug, Default)]
pub struct Report {
    pub(crate) results: Vec<CheckResult>,
}

impl Report {
    /// True when no check of the report is down.
    pub fn is_up(&self) -> bool {
        self.results.iter().all(|r| r.health.is_up())
    }

    pub fn results(&self) -> &[CheckResult] {
        &self.results
    }
}

type InFlight = Shared<BoxFuture<'static, Arc<Report>>>;

/// Runs the checks concurrently on `Dep<dyn Timer>`, each under its bound. A probe arriving while a
/// run is in flight awaits that run; nothing is kept once it ends.
pub(crate) struct Runner {
    pub(crate) timer: Arc<dyn Timer>,
    pub(crate) checks: Vec<Arc<dyn Check>>,
    pub(crate) in_flight: Mutex<Option<InFlight>>,
}

impl Runner {
    pub(crate) fn new(timer: Arc<dyn Timer>, checks: Vec<Arc<dyn Check>>) -> Self {
        Runner { timer, checks, in_flight: Mutex::new(None) }
    }

    /// The report of the checks belonging to `probe`.
    pub(crate) async fn run(&self, probe: Probe) -> Arc<Report> {
        let _ = probe;
        todo!()
    }
}
