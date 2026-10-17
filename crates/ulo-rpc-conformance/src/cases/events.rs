//! Scenarios: events.

use ulo_rpc::Link;
use ulo_transport::Count;

use crate::Broker;
use crate::rt::sleep;
use crate::cases::app::{EVENT, Fixture, addresses, client_over, eventually, ready, server_over};
use crate::cases::deadlines::{MARK, Stamped, Tap, Tapped};

/// An event reaches its handler, and nothing answers it.
pub async fn reaches_handler<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let settle = fixture.broker.budget().settle;
    fixture.rpc().emit(EVENT, &7u32).await.expect("the event is published");
    let probe = fixture.server.probe.clone();
    assert!(eventually::<B>(settle, || probe.events() == 1).await, "the event did not reach its handler within {settle:?}");
    assert_eq!(probe.last_event(), Some(7));
    // A second settle catches a redelivery or a second instance's copy.
    sleep::<B>(settle).await;
    assert_eq!(probe.events(), 1, "the event reached its handler more than once");
    fixture.stop().await;
}

/// An event the server refuses is settled so its broker never hands it over again: one whose
/// `deadline-ms` the server cannot read, written by a [`Stamped`] link, is rejected, and the
/// server's own link has delivered it once two settle budgets later, its handler never run. On a
/// link declaring `holds_unserved`, whose broker hands what it holds to the next instance, the next
/// instance in the server's place is handed an event emitted once it serves, and that alone: the
/// rejection settled the refused event at the broker, not only at the first instance.
pub async fn refused_event_is_not_redelivered<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    let budget = fixture.broker.budget();
    let link = Stamped { link: fixture.broker.client_link(&addresses(&[&fixture.server])), value: "abc" };
    let stamped = client_over::<B, _>(link).await;
    stamped.rpc.emit(EVENT, &1u32).header(MARK, "refused").await.expect("the refused event is published");
    assert!(eventually::<B>(budget.boot + budget.settle, || !tap.marks().is_empty()).await, "the refused event never reached the server");
    // Two settle budgets catch a broker handing a rejected delivery back.
    sleep::<B>(budget.settle * 2).await;
    assert_eq!(tap.marks(), vec!["refused"], "the marked events the server's link delivered");
    assert_eq!(fixture.server.probe.events(), 0, "the refused event's handler ran");
    stamped.close().await;

    let Fixture { broker, server, client } = fixture;
    server.stop().await;
    if broker.link().capabilities().holds_unserved {
        let next_tap = Tap::default();
        let next = server_over(&broker, Tapped { link: broker.link(), tap: next_tap.clone() }, Count::Default).await;
        ready(&broker, &client.rpc, &[&next]).await;
        client.rpc.emit(EVENT, &2u32).header(MARK, "after").await.expect("an event to the next instance is published");
        let delivered = eventually::<B>(budget.boot + budget.settle, || next_tap.marks().iter().any(|mark| mark == "after")).await;
        assert!(delivered, "the event emitted once the next instance served never reached it");
        sleep::<B>(budget.settle).await;
        assert_eq!(next_tap.marks(), vec!["after"], "the marked events the next instance's link delivered");
        next.stop().await;
    }
    client.close().await;
}
