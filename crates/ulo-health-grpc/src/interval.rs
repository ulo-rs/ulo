use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use tonic_health::ServingStatus;
use ulo::{Bound, Runtime};
use ulo_grpc::GrpcHealth;
use ulo_health::Readiness;

/// `Bound::Default`'s interval (integrations DESIGN §3.4, decision 13).
const DEFAULT_INTERVAL: Duration = Duration::from_secs(5);

/// The bridge's interval was `Bound::After(Duration::ZERO)`, which would evaluate continuously.
#[derive(Debug)]
pub struct ZeroInterval;

impl fmt::Display for ZeroInterval {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(
            "`HealthGrpc::interval(Bound::After(Duration::ZERO))` would re-evaluate readiness without pause; \
             write `Bound::Unbounded` to change the gRPC health status at lifecycle transitions alone",
        )
    }
}

impl Error for ZeroInterval {}

/// Refuses a zero interval.
pub(crate) fn validate(interval: Bound) -> Result<(), ZeroInterval> {
    if interval == Bound::After(Duration::ZERO) { Err(ZeroInterval) } else { Ok(()) }
}

/// The wait between two evaluations; `None` for `Unbounded`, which evaluates once.
fn period(interval: Bound) -> Option<Duration> {
    match interval {
        Bound::Default => Some(DEFAULT_INTERVAL),
        Bound::After(every) => Some(every),
        Bound::Unbounded => None,
    }
}

/// `""` as readiness answers it now.
async fn overall(readiness: &Readiness) -> ServingStatus {
    if readiness.readiness().await.is_up() { ServingStatus::Serving } else { ServingStatus::NotServing }
}

/// The re-evaluation task on the app's runtime: writes `""` through `health` whenever readiness
/// differs from the last status, until the app stops.
///
/// The first evaluation runs at once, so `""` follows readiness from the start; `Unbounded` stops
/// after it. Every write goes through `GrpcHealth`'s stored copy, which reaches the reporter only
/// on a change, so an unchanged tick sends a `Watch` subscriber nothing. The bridge's
/// before-shutdown hook aborts the task before it writes `NOT_SERVING`, which leaves that write
/// the last; the task's own end at `Stopping` covers a hook the shutdown cap skipped.
pub(crate) async fn reevaluate(
    runtime: Arc<dyn Runtime>,
    readiness: Readiness,
    health: GrpcHealth,
    interval: Bound,
) {
    let every = period(interval);
    loop {
        // Read before the evaluation: a run that started before `Stopping` can answer up after it,
        // and only a run that started after it is known to answer down. Readiness never recovers
        // from `Stopping`, so that write is the task's last.
        let stopping = readiness.is_stopping();
        let status = overall(&readiness).await;
        health.set_named("", status).await;
        if stopping {
            return;
        }
        match every {
            Some(every) => ulo::Timer::sleep(&*runtime, every).await,
            // `Stopping` arrived mid-run: one more pass writes the down status.
            None if readiness.is_stopping() => {}
            None => return,
        }
    }
}
