use std::error::Error;
use std::fmt;
use std::sync::Arc;

use ulo::{Bound, Runtime};
use ulo_grpc::GrpcHealth;
use ulo_health::Readiness;

/// The bridge's interval was `Bound::After(Duration::ZERO)`, which would evaluate continuously.
#[derive(Debug)]
pub struct ZeroInterval;

impl fmt::Display for ZeroInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("the gRPC health interval is zero; use Bound::Unbounded for lifecycle transitions alone")
    }
}

impl Error for ZeroInterval {}

/// Refuses a zero interval.
pub(crate) fn validate(interval: Bound) -> Result<(), ZeroInterval> {
    let _ = interval;
    todo!()
}

/// The re-evaluation task on the app's runtime: writes `""` through `health` whenever readiness
/// differs from the last status, until the app stops.
pub(crate) async fn reevaluate(
    runtime: Arc<dyn Runtime>,
    readiness: Readiness,
    health: GrpcHealth,
    interval: Bound,
) {
    let _ = (runtime, readiness, health, interval);
    todo!()
}
