use ulo::{Bound, Module, ModuleDef, ModuleIdentity};

/// The bridge module. Its init hook calls `GrpcHealth::delegate_overall()` before `listen()`, then
/// writes `""` from `Readiness` through `Dep<GrpcHealth>`: `NOT_SERVING` at `Stopping` and whenever
/// a check is down, `SERVING` otherwise.
///
/// The application imports `GrpcHealth::module()` beside it.
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
    /// lifecycle transitions alone. `Bound::After(Duration::ZERO)` is refused at `connect`, as
    /// `ping_interval`'s zero is, with [`ZeroInterval`](crate::ZeroInterval).
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
        let _ = m;
        todo!()
    }
}
