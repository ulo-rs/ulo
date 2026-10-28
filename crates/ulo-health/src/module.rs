use std::error::Error;
use std::fmt;
use std::time::Duration;

use ulo::{Bound, Dep, Many, Module, ModuleDef, ModuleIdentity, Signal, Timer};

use crate::check::Check;
use crate::readiness::Readiness;
use crate::run::Runner;

/// What the before-shutdown hook's bound allows beyond the lag (integrations DESIGN §3.2).
const LAG_MARGIN: Duration = Duration::from_secs(1);

/// The module that reads `Many<dyn Check>`, binds [`Readiness`](crate::Readiness) and exports it.
///
/// Import it once, from any module: its export is global, so `ulo-health-http`, `ulo-health-grpc`
/// and the application read `Dep<Readiness>` without importing it again. It reads `Dep<dyn Timer>`,
/// which runs every check's bound and the lag, so the app needs a `Timer`.
///
/// A check declaring `Bound::After(Duration::ZERO)` fails `connect` with [`ZeroBound`] as the
/// binding's construction error, `connect` being the first point a check's `bound` can be read.
///
/// Its `BeforeApplicationShutdown` hook turns readiness false, then waits the [`lag`](Self::lag),
/// bounded `After(lag + 1 s)` so the core's 10 s `hook_timeout` does not cut a longer lag; a
/// `Timer` that refuses the lag, its runtime shut down, ends the hook at once. The hook
/// belongs to the binding, which is built after every check it reads, so in the reverse connect
/// order of the before-shutdown stage it runs ahead of every check's hooks; a lazily loaded
/// module's hooks run ahead of it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthModule {
    lag: Duration,
}

impl HealthModule {
    pub fn new() -> Self {
        HealthModule { lag: Duration::ZERO }
    }

    /// How long the before-shutdown hook waits after turning readiness false, zero by default.
    ///
    /// The wait is the time kube-proxy and the load balancers take to see the endpoint change
    /// before the app refuses work. Kubernetes marks a pod's endpoints terminating before its
    /// `preStop` hook runs, so with a `preStop` sleep leave the lag at zero, and without one set it
    /// to a few seconds. Zero means no wait, and is not refused. A lag reaching past the builder's
    /// `shutdown_timeout` is cut by that cap and recorded `TimedOut { limit: ShutdownCap }`; the
    /// cap is not visible at registration, so nothing refuses it there.
    pub fn lag(mut self, lag: Duration) -> Self {
        self.lag = lag;
        self
    }
}

impl Default for HealthModule {
    fn default() -> Self {
        HealthModule::new()
    }
}

impl Module for HealthModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let lag = self.lag;
        m.global();
        m.try_singleton(|timer: Dep<dyn Timer>, checks: Many<dyn Check>| async move { build(timer, checks) })
            .before_shutdown(move |_signal: Signal, readiness: Dep<Readiness>| async move {
                readiness.stop();
                if !lag.is_zero()
                    && let Err(refused) = readiness.shared.runner.timer.try_sleep(lag).await
                {
                    tracing::debug!(%refused, "the app's `Timer` refused the health module's lag; the shutdown goes on without it");
                }
            })
            .timeout(lag.saturating_add(LAG_MARGIN));
        m.export::<Readiness>();
    }
}

fn build(timer: Dep<dyn Timer>, checks: Many<dyn Check>) -> Result<Readiness, ZeroBound> {
    if let Some(check) = checks.iter().find(|check| check.bound() == Bound::After(Duration::ZERO)) {
        return Err(ZeroBound { check: check.name().to_owned() });
    }
    Ok(Readiness::new(Runner::new(timer.into_arc(), checks.iter().cloned().collect())))
}

/// A check declared `Bound::After(Duration::ZERO)`, refused at `connect` where the health module
/// reads the collection.
#[derive(Debug)]
pub struct ZeroBound {
    pub(crate) check: String,
}

impl ZeroBound {
    /// The name of the check that declared the zero bound.
    pub fn check(&self) -> &str {
        &self.check
    }
}

impl fmt::Display for ZeroBound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the health check `{}` declares a zero bound, which would mark it down before it ran", self.check)
    }
}

impl Error for ZeroBound {}
