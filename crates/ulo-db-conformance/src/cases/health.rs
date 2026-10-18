use std::time::Duration;

use ulo_health::HealthModule;

use crate::Host;
use crate::cases::{Root, builder, closed, connected, describe, readiness, readiness_reads, store};

/// A check's own bound, 2 s by default. A ping waiting on a pool's acquire answers only at that
/// bound, and the wait reads its deadline between runs alone, so it allows two of them.
const CHECK_BOUND: Duration = Duration::from_secs(2);

/// The health check goes down in an outage and recovers with the server. Planted break: a check that
/// reports a cached `Up`.
///
/// The store module and the health module connect against the host's options, and readiness reads
/// true with the store's check up. Once the host cuts the store off, readiness turns false with the
/// store's check down within the settle budget and two of the check's bounds. Once it restores the
/// store, both turn up again within the recovery budget. No result is kept between runs, so a check that
/// answers from a cached `Up` reads up through the outage.
pub async fn outage_and_recovery<H: Host>() {
    let host = H::start().await;
    let budget = host.budget();
    let app = connected(builder::<H>(Root::new().import(store::<H>(host.options())).import(HealthModule::new())), "the app").await;
    let readiness = readiness(&app).await;

    let (up, before) = readiness_reads::<H>(&readiness, true, budget.settle).await;
    assert!(up, "readiness did not read true with the store reachable: {}", describe(&before));

    host.cut().await;
    let within = budget.settle + CHECK_BOUND + CHECK_BOUND;
    let (down, during) = readiness_reads::<H>(&readiness, false, within).await;
    assert!(down, "readiness did not turn false within {within:?} of the store's outage: {}", describe(&during));

    host.restore().await;
    let (recovered, after) = readiness_reads::<H>(&readiness, true, budget.recovery).await;
    assert!(recovered, "readiness did not recover within {:?} of the store's restore: {}", budget.recovery, describe(&after));
    closed(app, "the app").await;
}
