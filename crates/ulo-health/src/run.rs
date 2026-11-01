use std::error::Error;
use std::fmt;
use std::panic::AssertUnwindSafe;
use std::pin::pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::{Either, Shared, join_all, select};
use ulo::{BoxFuture, Bound, Timer};

use crate::check::{Check, Health, Probe};

/// What `Bound::Default` means for a check (integrations DESIGN decision 16).
const DEFAULT_BOUND: Duration = Duration::from_secs(2);

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

/// A run in flight and the probes awaiting it. The last of them to leave clears the slot, whether
/// the run finished or every caller gave up on it: a run nobody awaits is dropped rather than
/// resumed later, when its checks' bounds, measured from its start, would already have passed.
struct Run {
    id: u64,
    future: InFlight,
    waiters: usize,
}

#[derive(Default)]
struct Slots {
    next: u64,
    readiness: Option<Run>,
    liveness: Option<Run>,
}

impl Slots {
    fn of(&mut self, probe: Probe) -> &mut Option<Run> {
        match probe {
            Probe::Readiness => &mut self.readiness,
            Probe::Liveness => &mut self.liveness,
        }
    }
}

/// Runs the checks concurrently on `Dep<dyn Timer>`, each under its bound. A probe arriving while a
/// run is in flight awaits that run; nothing is kept once it ends.
pub(crate) struct Runner {
    pub(crate) timer: Arc<dyn Timer>,
    readiness: Arc<[Arc<dyn Check>]>,
    liveness: Arc<[Arc<dyn Check>]>,
    /// Held only to join or leave a run, never across an await.
    in_flight: Mutex<Slots>,
}

impl Runner {
    pub(crate) fn new(timer: Arc<dyn Timer>, checks: Vec<Arc<dyn Check>>) -> Self {
        let (liveness, readiness): (Vec<_>, Vec<_>) = checks.into_iter().partition(|check| check.probe() == Probe::Liveness);
        Runner { timer, readiness: readiness.into(), liveness: liveness.into(), in_flight: Mutex::new(Slots::default()) }
    }

    /// The report of the checks belonging to `probe`.
    pub(crate) async fn run(&self, probe: Probe) -> Arc<Report> {
        let (id, future) = self.join(probe);
        let _waiter = Waiter { runner: self, probe, id };
        future.await
    }

    /// The run in flight for `probe`, or a new one when none is, or when the one in the slot has
    /// finished: a finished run's report is never handed to a probe that arrived after it.
    fn join(&self, probe: Probe) -> (u64, InFlight) {
        let mut slots = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        let id = slots.next;
        let slot = slots.of(probe);
        if let Some(run) = slot.as_mut()
            && run.future.peek().is_none()
        {
            run.waiters += 1;
            return (run.id, run.future.clone());
        }
        let future = self.start(probe).shared();
        let replaced = slot.replace(Run { id, future: future.clone(), waiters: 1 });
        slots.next = id.wrapping_add(1);
        drop(slots);
        // A replaced run has finished; its waiters hold their own clones.
        drop(replaced);
        (id, future)
    }

    fn start(&self, probe: Probe) -> BoxFuture<'static, Arc<Report>> {
        let checks = match probe {
            Probe::Readiness => Arc::clone(&self.readiness),
            Probe::Liveness => Arc::clone(&self.liveness),
        };
        let timer = Arc::clone(&self.timer);
        Box::pin(async move {
            let results = join_all(checks.iter().map(|check| run_one(&**check, &*timer))).await;
            Arc::new(Report { results })
        })
    }

    fn leave(&self, probe: Probe, id: u64) {
        let mut slots = self.in_flight.lock().unwrap_or_else(PoisonError::into_inner);
        let slot = slots.of(probe);
        let last = match slot.as_mut() {
            Some(run) if run.id == id => {
                run.waiters -= 1;
                run.waiters == 0
            }
            _ => false,
        };
        let removed = if last { slot.take() } else { None };
        drop(slots);
        // Dropping the last handle on an unfinished run drops the checks' futures, which run
        // their own code; that happens outside the lock.
        drop(removed);
    }
}

/// One probe's place in a run, released when the probe's future completes or is dropped.
struct Waiter<'a> {
    runner: &'a Runner,
    probe: Probe,
    id: u64,
}

impl Drop for Waiter<'_> {
    fn drop(&mut self) {
        self.runner.leave(self.probe, self.id);
    }
}

async fn run_one(check: &dyn Check, timer: &dyn Timer) -> CheckResult {
    let health = match check.bound() {
        Bound::Unbounded => answer(check).await,
        Bound::Default => within(timer, DEFAULT_BOUND, answer(check)).await,
        Bound::After(after) => within(timer, after, answer(check)).await,
    };
    CheckResult { name: check.name().to_owned(), probe: check.probe(), health }
}

/// The check's answer, a panic in it, or in the call that builds its future, read as down: one
/// check's fault never takes the probe, or the other checks of its run, with it.
async fn answer(check: &dyn Check) -> Health {
    match AssertUnwindSafe(async { check.check().await }).catch_unwind().await {
        Ok(health) => health,
        Err(_) => {
            tracing::warn!(check = check.name(), "a health check panicked; it is down for this run");
            Health::Down(Box::new(Panicked))
        }
    }
}

/// `health` raced against the bound. The check is polled first, so an answer arriving in the poll
/// the bound expires in counts.
async fn within(timer: &dyn Timer, after: Duration, health: impl Future<Output = Health>) -> Health {
    let health = pin!(health);
    match select(health, timer.sleep(after)).await {
        Either::Left((health, _)) => health,
        Either::Right(((), _)) => Health::Down(Box::new(NoAnswer { after })),
    }
}

#[derive(Debug)]
struct NoAnswer {
    after: Duration,
}

impl fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the check did not answer within {:?}", self.after)
    }
}

impl Error for NoAnswer {}

/// The panic's payload is left out: it is the check's own text, which nothing has redacted.
#[derive(Debug)]
struct Panicked;

impl fmt::Display for Panicked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the check panicked")
    }
}

impl Error for Panicked {}
