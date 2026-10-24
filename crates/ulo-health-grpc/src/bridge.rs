use std::convert::Infallible;
use std::sync::{Arc, Mutex, PoisonError};

use tonic_health::ServingStatus;
use ulo::{Bound, Dep, Module, ModuleDef, ModuleIdentity, Runtime, Signal, TaskEnd, TaskHandle};
use ulo_grpc::GrpcHealth;
use ulo_health::Readiness;

use crate::interval;

/// The bridge module. Its init hook calls `GrpcHealth::delegate_overall()` before `listen()`, then
/// writes `""` from `Readiness` through `Dep<GrpcHealth>`: `NOT_SERVING` at `Stopping` and whenever
/// a check is down, `SERVING` otherwise.
///
/// It imports `GrpcHealth::module()` itself, so the gRPC server and the bridge share one reporter
/// whether or not the application imports it too; two imports are one module. `Readiness` comes
/// from `ulo_health::HealthModule`, which the application imports.
///
/// `""` turns `NOT_SERVING` in the bridge's before-shutdown hook, which runs at `Stopping`, before
/// the drain's GOAWAY, so a client polling health stops sending first. This departs from
/// transports §6.2, which keeps `""` `SERVING` until the drain; without the bridge §6.2 holds. The
/// server's own drain write is a floor under it: with the hook skipped by a spent shutdown cap, or
/// no interval and so no task to end at `Stopping`, `""` still reads `NOT_SERVING` once the drain
/// begins.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HealthGrpc {
    pub(crate) interval: Bound,
}

impl HealthGrpc {
    /// Re-evaluates every 5 s.
    pub fn new() -> Self {
        HealthGrpc { interval: Bound::Default }
    }

    /// How often readiness is re-evaluated, since gRPC's `Check` reads a stored status and not a
    /// fresh evaluation. `Bound::Default` is 5 s and `Bound::Unbounded` leaves statuses changing at
    /// lifecycle transitions alone. `Bound::After(Duration::ZERO)` is refused by `wire()`, where the
    /// module's registration reads the setting, with [`ZeroInterval`](crate::ZeroInterval), as
    /// `ping_interval`'s zero is refused.
    pub fn interval(mut self, interval: Bound) -> Self {
        self.interval = interval;
        self
    }
}

impl Default for HealthGrpc {
    fn default() -> Self {
        HealthGrpc::new()
    }
}

impl Module for HealthGrpc {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(self)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(GrpcHealth::module());
        if let Err(zero) = interval::validate(self.interval) {
            // Recorded for `wire()` to report beside every other wiring error; nothing else of the
            // bridge is registered.
            m.try_value::<Bridge, _>(Err(zero));
            return;
        }
        let interval = self.interval;
        m.singleton(move |readiness: Dep<Readiness>, health: Dep<GrpcHealth>, runtime: Dep<dyn Runtime>| async move {
            Bridge {
                readiness: (*readiness).clone(),
                health: (*health).clone(),
                runtime: runtime.into_arc(),
                interval,
                task: Mutex::new(None),
            }
        });
        m.on_init(|bridge: Dep<Bridge>| async move {
            bridge.start();
            Ok::<(), Infallible>(())
        });
        m.before_shutdown(|_signal: Signal, bridge: Dep<Bridge>| async move { bridge.stop().await });
    }
}

/// The bridge's state: what the task writes through and the task itself. It keeps no status of its
/// own; `GrpcHealth`'s stored copy is the one record per name.
pub(crate) struct Bridge {
    readiness: Readiness,
    health: GrpcHealth,
    runtime: Arc<dyn Runtime>,
    interval: Bound,
    /// Filled by the init hook, emptied by the before-shutdown hook.
    task: Mutex<Option<TaskHandle>>,
}

impl Bridge {
    /// Hands `""` to the bridge and starts the re-evaluation task, which makes the first write.
    ///
    /// Inside the init hook, so `delegate_overall` precedes `listen()` and every write the server
    /// makes. The first evaluation runs on the task rather than in the hook: a check that never
    /// answers would otherwise fail `connect` on the hook's bound, and until the task writes, `""`
    /// holds `SERVING`, the reporter's start and what the server reports without the bridge.
    fn start(&self) {
        self.health.delegate_overall();
        let reevaluation = interval::reevaluate(
            Arc::clone(&self.runtime),
            self.readiness.clone(),
            self.health.clone(),
            self.interval,
        );
        let task = ulo::Spawn::spawn(&*self.runtime, Box::pin(reevaluation));
        if let Some(earlier) = self.task.lock().unwrap_or_else(PoisonError::into_inner).replace(task) {
            earlier.abort();
        }
    }

    /// Ends the task, then writes `NOT_SERVING`.
    ///
    /// The task is awaited to its end before the write: an evaluation that began before
    /// `Stopping` can answer up, and its write after this one would undo it.
    async fn stop(&self) {
        let task = self.task.lock().unwrap_or_else(PoisonError::into_inner).take();
        if let Some(task) = task {
            task.abort();
            if let TaskEnd::Panicked(message) = task.await {
                tracing::warn!(%message, "the gRPC health re-evaluation task panicked, and the overall status stopped following readiness there");
            }
        }
        self.health.set_named("", ServingStatus::NotServing).await;
    }
}

/// An app whose `connect` fails after the init hook, or that is dropped without a shutdown, runs
/// no before-shutdown hook; dropping its singletons ends the task here.
impl Drop for Bridge {
    fn drop(&mut self) {
        if let Some(task) = self.task.get_mut().unwrap_or_else(PoisonError::into_inner).take() {
            task.abort();
        }
    }
}
