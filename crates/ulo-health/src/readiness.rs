use std::error::Error;
use std::fmt;
use std::future::poll_fn;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use crate::check::{Health, Probe};
use crate::run::{CheckResult, Report, Runner};

/// The app's health as the probes read it, bound by [`HealthModule`](crate::HealthModule) and
/// injected as `Dep<Readiness>`.
///
/// Liveness is true whenever the process answers, plus any check declared `Probe::Liveness`.
/// Readiness is true while the app is `Running` and every readiness check is up, and false from
/// `Stopping`. Startup is true once `listen` has finished, which holds whenever a probe is answered.
///
/// `Running` is read from the health module's own state: readiness answers from its checks from
/// the moment the binding is built until the module's before-shutdown hook records `Stopping`.
/// Nothing serves a probe before `listen` has finished, so a probe never sees the state before it.
#[derive(Clone)]
pub struct Readiness {
    pub(crate) shared: Arc<State>,
}

pub(crate) struct State {
    pub(crate) runner: Runner,
    pub(crate) stopping: AtomicBool,
    /// Bumped when `stopping` flips, which [`Changes`] waits on.
    pub(crate) generation: AtomicU64,
    listeners: Mutex<Listeners>,
}

/// The wakers of the [`Changes`] waiting for the next bump, one entry per subscriber.
#[derive(Default)]
struct Listeners {
    next: u64,
    wakers: Vec<(u64, Waker)>,
}

impl State {
    fn listen(&self, id: u64, waker: &Waker) {
        let mut listeners = self.listeners.lock().unwrap_or_else(PoisonError::into_inner);
        match listeners.wakers.iter_mut().find(|(owner, _)| *owner == id) {
            Some((_, slot)) => {
                if !slot.will_wake(waker) {
                    slot.clone_from(waker);
                }
            }
            None => listeners.wakers.push((id, waker.clone())),
        }
    }

    fn forget(&self, id: u64) {
        let mut listeners = self.listeners.lock().unwrap_or_else(PoisonError::into_inner);
        listeners.wakers.retain(|(owner, _)| *owner != id);
    }
}

impl Readiness {
    pub(crate) fn new(runner: Runner) -> Self {
        Readiness {
            shared: Arc::new(State {
                runner,
                stopping: AtomicBool::new(false),
                generation: AtomicU64::new(0),
                listeners: Mutex::new(Listeners::default()),
            }),
        }
    }

    pub async fn liveness(&self) -> Arc<Report> {
        self.shared.runner.run(Probe::Liveness).await
    }

    /// Runs the readiness checks, or answers a report that is down without running them once the
    /// app has reached `Stopping`.
    pub async fn readiness(&self) -> Arc<Report> {
        if self.is_stopping() {
            return stopping();
        }
        let report = self.shared.runner.run(Probe::Readiness).await;
        // A run that began before the flip answers after it as the flip does, so no probe reads
        // ready once the before-shutdown hook has recorded `Stopping`.
        if self.is_stopping() { stopping() } else { report }
    }

    pub async fn startup(&self) -> Arc<Report> {
        Arc::new(Report::default())
    }

    /// Whether the app has reached `Stopping`, as the before-shutdown hook recorded.
    pub fn is_stopping(&self) -> bool {
        self.shared.stopping.load(Ordering::Acquire)
    }

    /// A notice that readiness may have changed, for the gRPC bridge: it resolves when the app
    /// reaches `Stopping`.
    ///
    /// A check's result is not a change it reports: no result is kept between runs, so a check
    /// going down is seen by the next run, which the bridge makes on its interval. A subscription
    /// taken after `Stopping` resolves at once.
    pub fn subscribe(&self) -> Changes {
        let id = {
            let mut listeners = self.shared.listeners.lock().unwrap_or_else(PoisonError::into_inner);
            let id = listeners.next;
            listeners.next = id.wrapping_add(1);
            id
        };
        Changes { shared: Arc::clone(&self.shared), seen: 0, id }
    }

    /// Records `Stopping` and wakes every subscriber; a second call changes nothing.
    pub(crate) fn stop(&self) {
        if self.shared.stopping.swap(true, Ordering::AcqRel) {
            return;
        }
        // The bump precedes taking the wakers, and a subscriber reads the generation after
        // registering its waker, so a subscriber polled concurrently either is woken here or
        // reads the bump itself.
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let wakers = {
            let mut listeners = self.shared.listeners.lock().unwrap_or_else(PoisonError::into_inner);
            std::mem::take(&mut listeners.wakers)
        };
        tracing::debug!("readiness is false: the app is stopping");
        for (_, waker) in wakers {
            waker.wake();
        }
    }
}

/// Resolves each time the readiness state changes after the last notice it delivered.
pub struct Changes {
    pub(crate) shared: Arc<State>,
    pub(crate) seen: u64,
    id: u64,
}

impl Changes {
    pub async fn changed(&mut self) {
        poll_fn(|cx| self.poll_changed(cx)).await;
    }

    fn poll_changed(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        if self.take_change() {
            return Poll::Ready(());
        }
        self.shared.listen(self.id, cx.waker());
        if self.take_change() { Poll::Ready(()) } else { Poll::Pending }
    }

    fn take_change(&mut self) -> bool {
        let now = self.shared.generation.load(Ordering::Acquire);
        if now == self.seen {
            return false;
        }
        self.seen = now;
        true
    }
}

impl Drop for Changes {
    fn drop(&mut self) {
        self.shared.forget(self.id);
    }
}

/// The name of the one result a report answered from `Stopping` carries.
const LIFECYCLE: &str = "lifecycle";

fn stopping() -> Arc<Report> {
    Arc::new(Report {
        results: vec![CheckResult {
            name: LIFECYCLE.to_owned(),
            probe: Probe::Readiness,
            health: Health::Down(Box::new(Stopping)),
        }],
    })
}

#[derive(Debug)]
struct Stopping;

impl fmt::Display for Stopping {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the app is stopping")
    }
}

impl Error for Stopping {}
