use std::time::Duration;

use ulo_db::Store;
use ulo_health::HealthModule;

use crate::cases::{PATIENT, Root, bounded, builder, closed, connecting, describe, readiness, readiness_reads, store_reads};
use crate::{Host, report};

/// How long a lazy start may take with the store out: it builds the client and waits for nothing.
const START: Duration = Duration::from_secs(5);

/// `.lazy()` keeps readiness false until the first ping. Planted break: readiness set true before
/// the first ping.
///
/// With the host's options cut, a lazy module and the health module start: the connect succeeds in
/// under five seconds, well inside the module's twenty-second bound, since a lazy start waits for
/// no ping. Readiness then reads false and the store's check reads down, at once and for as long as
/// the outage lasts, because no ping has succeeded. Restored, the store's check and readiness turn
/// up within the host's recovery budget. A check that answers up before its first successful ping
/// reads up during the outage.
pub async fn waits_for_the_first_ping<H: Host>() {
    let host = H::start().await;
    assert!(
        host.capabilities().lazy,
        "the host declares `{}` without `.lazy()`: declare this scenario not applicable",
        <H::Store as Store>::NAME
    );
    assert!(
        !<H::Store as Store>::CREATE_CONNECTS,
        "the host declares `.lazy()` for `{}`, whose `create` connects, and which therefore offers none",
        <H::Store as Store>::NAME
    );
    let budget = host.budget();

    host.cut().await;
    let module = host.lazily(bounded::<H>(host.options(), PATIENT));
    let (outcome, took) =
        connecting(builder::<H>(Root::new().import(module).import(HealthModule::new())), "a lazy app with the store out").await;
    let app = outcome.unwrap_or_else(|error| {
        panic!("a lazy app with the store out was refused after {took:?}; a lazy start waits for no ping: {}", report(&error))
    });
    assert!(took < START, "a lazy app with the store out took {took:?} to connect: the start waited for the store");

    let readiness = readiness(&app).await;
    let first = readiness.readiness().await;
    assert!(!first.is_up(), "readiness read true before the store's first successful ping: {}", describe(&first));
    assert_eq!(
        store_reads::<H::Store>(&first, false),
        Some(true),
        "the store's check did not read down before its first successful ping: {}",
        describe(&first)
    );
    let (turned_up, during) = readiness_reads::<H>(&readiness, true, budget.settle).await;
    assert!(!turned_up, "readiness turned true with the store still out: {}", describe(&during));

    host.restore().await;
    let (up, after) = readiness_reads::<H>(&readiness, true, budget.recovery).await;
    assert!(up, "readiness did not turn true within {:?} of the store's restore: {}", budget.recovery, describe(&after));
    closed(app, "the lazy app").await;
}
