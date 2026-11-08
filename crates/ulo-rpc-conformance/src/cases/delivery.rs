//! Scenarios: delivery.

use ulo_rpc::{DeliveryMode, Link};

use crate::Broker;
use crate::rt::sleep;
use crate::cases::app::{ADD, Add, EVERY, Mounts, Server, Sum, TALLY, client, eventually, ready, refusal_at_startup, server};

/// Events the scenarios publish.
const EVENTS: usize = 10;

/// Requests the request scenario sends.
const REQUESTS: usize = 10;

/// Two server instances on one event pattern, the event left at its default: each event reaches
/// one of them, so the two handle `EVENTS` between them. Under `Addressed` the caller names one
/// server, and a second cannot share its address, so the scenario runs one and asserts it received
/// every event. Skipped on a link declared without `single_delivery`, printing its reason: there
/// each instance receives every event, as each receives one declared `every_instance`.
pub async fn one_instance<B: Broker>() {
    let broker = B::start().await;
    let capabilities = broker.link().capabilities();
    if let Some(reason) = capabilities.why_not_single_delivery() {
        crate::skipped("an_event_reaches_one_instance", "single_delivery", reason);
        return;
    }
    let delivery = capabilities.delivery;
    let first = server(&broker).await;
    let second = if delivery == DeliveryMode::Addressed { None } else { Some(server(&broker).await) };
    let servers: Vec<&Server> = std::iter::once(&first).chain(second.as_ref()).collect();
    let caller = client(&broker, &servers).await;
    ready(&broker, &caller.rpc, &servers).await;
    let budget = broker.budget();
    // The second instance's subscription settles before the events are published.
    sleep::<B>(budget.settle).await;

    for event in 0..EVENTS {
        caller.rpc.emit(TALLY, &(event as u32)).await.expect("the event is published");
    }
    let total = || first.probe.tallies() + second.as_ref().map_or(0, |second| second.probe.tallies());
    assert!(
        eventually::<B>(budget.boot + budget.settle, || total() >= EVENTS).await,
        "{} of {EVENTS} deliveries arrived across {} instances",
        total(),
        servers.len()
    );
    sleep::<B>(budget.settle).await;
    assert_eq!(total(), EVENTS, "deliveries of {EVENTS} events left at one instance, across {} instances", servers.len());

    first.stop().await;
    if let Some(second) = second {
        second.stop().await;
    }
    caller.close().await;
}

/// Two server instances on one event pattern, the event declared `every_instance`: each event
/// reaches both, so each handles all `EVENTS`. On a link declared without `every_instance`, where
/// a caller addresses one server, a server mounting such a handler is refused at startup, and the
/// refusal names the option and the link's reason.
pub async fn every_instance<B: Broker>() {
    let broker = B::start().await;
    let capabilities = broker.link().capabilities();
    if let Some(reason) = capabilities.why_not_every_instance() {
        let refusal = refusal_at_startup(&broker, Mounts { every_instance: true, ..Mounts::carried(&capabilities) }).await;
        assert!(
            refusal.contains("every_instance") && refusal.contains(reason),
            "the refusal of an `every_instance` event on a link declared without it names neither the option nor the reason `{reason}`: {refusal}"
        );
        return;
    }
    let first = server(&broker).await;
    let second = server(&broker).await;
    let servers = [&first, &second];
    let caller = client(&broker, &servers).await;
    ready(&broker, &caller.rpc, &servers).await;
    let budget = broker.budget();
    // Both instances' subscriptions settle before the events are published.
    sleep::<B>(budget.settle).await;

    for event in 0..EVENTS {
        caller.rpc.emit(EVERY, &(event as u32)).await.expect("the event is published");
    }
    let each = || (first.probe.everyone(), second.probe.everyone());
    assert!(
        eventually::<B>(budget.boot + budget.settle, || each().0 >= EVENTS && each().1 >= EVENTS).await,
        "of {EVENTS} events declared `every_instance`, the two instances handled {:?}",
        each()
    );
    sleep::<B>(budget.settle).await;
    assert_eq!(each(), (EVENTS, EVENTS), "deliveries of {EVENTS} events declared `every_instance` to each of two instances");

    first.stop().await;
    second.stop().await;
    caller.close().await;
}

/// Two server instances answering requests on one pattern.
///
/// A link declaring `single_delivery` runs each request's handler on one instance; one
/// declaring it `false` runs it on both, the client answering the first reply. Under `Addressed`
/// the scenario runs one instance, as [`one_instance`] does, and asserts each request ran once.
pub async fn requests_across_two_instances<B: Broker>() {
    let broker = B::start().await;
    let capabilities = broker.link().capabilities();
    let first = server(&broker).await;
    let second = if capabilities.delivery == DeliveryMode::Addressed { None } else { Some(server(&broker).await) };
    let servers: Vec<&Server> = std::iter::once(&first).chain(second.as_ref()).collect();
    let caller = client(&broker, &servers).await;
    ready(&broker, &caller.rpc, &servers).await;
    let budget = broker.budget();
    // The second instance's subscription settles, and every reply to the readiness calls arrives,
    // before the count starts.
    sleep::<B>(budget.settle).await;
    let total = || first.probe.adds_begun() + second.as_ref().map_or(0, |second| second.probe.adds_begun());
    let before = total();

    for n in 0..REQUESTS as i64 {
        let sum = caller.rpc.request::<_, Sum>(ADD, &Add { a: n, b: 1 }).await.expect("the request is answered");
        assert_eq!(sum, Sum { sum: n + 1 });
    }
    let expected = if capabilities.single_delivery() { REQUESTS } else { servers.len() * REQUESTS };
    let ran = || total() - before;
    let once = capabilities.single_delivery();
    assert!(
        eventually::<B>(budget.boot + budget.settle, || ran() >= expected).await,
        "{} of {expected} executions ran across {} instances, `single_delivery: {once}`",
        ran(),
        servers.len()
    );
    sleep::<B>(budget.settle).await;
    assert_eq!(ran(), expected, "executions of {REQUESTS} requests across {} instances, `single_delivery: {once}`", servers.len());

    first.stop().await;
    if let Some(second) = second {
        second.stop().await;
    }
    caller.close().await;
}
