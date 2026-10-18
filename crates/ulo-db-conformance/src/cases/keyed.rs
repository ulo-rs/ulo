use std::sync::Arc;

use ulo::{Dep, ExecOptions, Execution, LookupError, Module};
use ulo_db::Store;
use ulo_health::HealthModule;

use crate::Host;
use crate::cases::{Client, Root, builder, closed, connected, describe, readiness, readiness_reads, store, store_checks};

/// The first keyed instance's qualifier.
struct First;

/// The second keyed instance's qualifier.
struct Second;

/// Two keyed instances stay separate: two pools and two checks. Planted break: two qualifiers
/// resolving to one instance.
///
/// Two modules over the host's options, one keyed `First` and one `Second`, and the health module
/// connect. Each qualified client resolves, to two distinct instances, and each answers a ping of
/// its own; the unqualified client is no binding, since a keyed module requalifies its exports; and
/// readiness reads true over two checks contributed by the store, one per instance.
pub async fn two_instances<H: Host>() {
    let host = H::start().await;
    let root = Root::new()
        .import(store::<H>(host.options()).keyed::<First>())
        .import(store::<H>(host.options()).keyed::<Second>())
        .import(HealthModule::new());
    let app = connected(builder::<H>(root), "the app with two keyed instances").await;

    let exec = Execution::open(&app.handle().root(), ExecOptions::new())
        .unwrap_or_else(|closed| crate::startup_failed!("a standalone execution was refused: {closed}"));
    let first: Dep<Client<H>, First> = exec
        .resolver()
        .dep_qualified::<Client<H>, First>()
        .await
        .unwrap_or_else(|error| panic!("the instance keyed `First` exports no client: {error}"));
    let second: Dep<Client<H>, Second> = exec
        .resolver()
        .dep_qualified::<Client<H>, Second>()
        .await
        .unwrap_or_else(|error| panic!("the instance keyed `Second` exports no client: {error}"));
    drop(exec);
    assert!(!Arc::ptr_eq(first.arc(), second.arc()), "the qualifiers `First` and `Second` resolve to one client");

    for (name, outcome) in [
        ("First", <H::Store as Store>::ping(&*first).await),
        ("Second", <H::Store as Store>::ping(&*second).await),
    ] {
        if let Err(error) = outcome {
            panic!("the client keyed `{name}` did not answer a ping: {}", crate::report(&*error));
        }
    }

    match app.get::<Client<H>>().await {
        Err(LookupError::NotFound { .. }) => {}
        Err(other) => panic!("the unqualified client beside two keyed instances is not a missing binding: {other}"),
        Ok(_) => panic!("an unqualified client is bound beside two keyed instances, whose exports are requalified"),
    }

    let readiness = readiness(&app).await;
    let (up, report) = readiness_reads::<H>(&readiness, true, host.budget().settle).await;
    assert!(up, "readiness did not read true over the two instances: {}", describe(&report));
    assert_eq!(
        store_checks::<H::Store>(&report),
        2,
        "the two keyed instances did not contribute a check each: {}",
        describe(&report)
    );
    closed(app, "the app with two keyed instances").await;
}
