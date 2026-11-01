//! Scenarios: delivery.

use ulo_rpc::{DeliveryMode, Link};

use crate::Broker;
use crate::rt::sleep;
use crate::cases::app::{ADD, Add, Server, Sum, TALLY, client, eventually, ready, server};

/// Events the scenario publishes.
const EVENTS: usize = 10;

/// Requests the request scenario sends.
const REQUESTS: usize = 10;

/// Two server instances under the link's declared `DeliveryMode`.
///
/// `Competing` delivers each event to one instance, `FanOut` to both. Under `Addressed` the caller
/// names one server, and a second cannot share its address, so the scenario runs one and asserts
/// it received every event.
pub async fn two_instances<B: Broker>() {
    let broker = B::start().await;
    let delivery = broker.link().capabilities().delivery;
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
    let expected = match delivery {
        DeliveryMode::FanOut => 2 * EVENTS,
        _ => EVENTS,
    };
    let total = || first.probe.tallies() + second.as_ref().map_or(0, |second| second.probe.tallies());
    assert!(
        eventually::<B>(budget.boot + budget.settle, || total() >= expected).await,
        "{} of {expected} deliveries arrived under {delivery:?}",
        total()
    );
    sleep::<B>(budget.settle).await;
    assert_eq!(total(), expected, "deliveries under {delivery:?}");

    first.stop().await;
    if let Some(second) = second {
        second.stop().await;
    }
    caller.close().await;
}

/// Two server instances answering requests on one pattern.
///
/// A link declaring `delivers_requests_once` runs each request's handler on one instance; one
/// declaring it `false` runs it on both, the client answering the first reply. Under `Addressed`
/// the scenario runs one instance, as [`two_instances`] does, and asserts each request ran once.
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
    let expected = if capabilities.delivers_requests_once { REQUESTS } else { servers.len() * REQUESTS };
    let ran = || total() - before;
    let once = capabilities.delivers_requests_once;
    assert!(
        eventually::<B>(budget.boot + budget.settle, || ran() >= expected).await,
        "{} of {expected} executions ran across {} instances, `delivers_requests_once: {once}`",
        ran(),
        servers.len()
    );
    sleep::<B>(budget.settle).await;
    assert_eq!(ran(), expected, "executions of {REQUESTS} requests across {} instances, `delivers_requests_once: {once}`", servers.len());

    first.stop().await;
    if let Some(second) = second {
        second.stop().await;
    }
    caller.close().await;
}
