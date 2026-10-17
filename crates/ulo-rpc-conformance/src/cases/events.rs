//! Scenarios: events.

use std::future::IntoFuture;
use std::time::{Duration, Instant};

use ulo::ExecOptions;
use ulo_rpc::{Link, RpcError};
use ulo_transport::{Count, ErrorKind};

use crate::{Broker, Budget};
use crate::rt::{runtime, sleep};
use crate::cases::app::{EVENT, Fixture, SLOW_EVENT, SlowEnd, SlowRun, addresses, client_over, eventually, ready, server_over, within};
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

/// The time the emitter's execution has left in the scenarios below, a tenth of the broker's
/// settle budget, 50 ms at the default budget.
fn left_of(budget: Budget) -> Duration {
    budget.settle / 10
}

/// How long the slow event's handler runs in the scenarios below: four times what the emitter's
/// execution has left, so a deadline carried from the emitter would end it.
fn runs_of(budget: Budget) -> Duration {
    left_of(budget) * 4
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Waits until a run of the slow event's handler has ended, and answers the first that has.
async fn ended_once<B: Broker>(runs: impl Fn() -> Vec<SlowRun>, wait: Duration, what: &str) -> SlowRun {
    let ended = eventually::<B>(wait, || runs().iter().any(|run| run.ended.is_some())).await;
    let seen = runs();
    assert!(ended, "{what}: the slow event's handler had not ended {wait:?} on, its runs: {seen:?}");
    seen.into_iter().find(|run| run.ended.is_some()).expect("a run that ended")
}

/// An event emitted `.within` an execution carries none of the execution's deadline: emitted from
/// one with a tenth of a settle budget left, to a handler running four times that, the event's
/// frame carries no `deadline-ms`, its handler's execution has no deadline, and the handler
/// completes. On a link declaring `holds_unserved` the event is emitted while no instance serves,
/// so the broker holds it past the emitter's deadline and hands it to the next instance.
pub async fn emitter_s_deadline_is_not_the_event_s<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    let budget = fixture.broker.budget();
    let (left, runs) = (left_of(budget), runs_of(budget));
    let holds = fixture.capabilities().holds_unserved;
    let Fixture { broker, server, client } = fixture;
    let (server, tap) = if holds {
        server.stop().await;
        (None, Tap::default())
    } else {
        (Some(server), tap)
    };
    let deadline = runtime::<B>().now() + left;
    let emitted = Instant::now();
    let sent = client
        .execute(ExecOptions::new().deadline(deadline), async |exec| {
            let event = client.rpc.emit(SLOW_EVENT, &millis(runs)).header(MARK, "emitted").within(&exec.handle());
            within::<B, _>(Duration::from_secs(10), "the event's publish", event.into_future()).await
        })
        .await;
    sent.expect("the event is published");
    let server = match server {
        Some(server) => server,
        None => {
            // Past the emitter's deadline before any instance takes the event.
            sleep::<B>(left * 2).await;
            let next = server_over(&broker, Tapped { link: broker.link(), tap: tap.clone() }, Count::Default).await;
            ready(&broker, &client.rpc, &[&next]).await;
            next
        }
    };
    let probe = server.probe.clone();
    let run = ended_once::<B>(|| probe.slow_runs(), budget.boot + budget.settle + runs, "the emitted event").await;
    let stamps = tap.stamps();
    assert_eq!(
        run.ended,
        Some(SlowEnd::Completed),
        "the event's handler, begun {:?} after the emit with {:?} left of its execution's deadline, ended {:?}; its frame carried `deadline-ms` {:?}",
        run.began.saturating_duration_since(emitted),
        run.left,
        run.ended,
        stamps,
    );
    assert_eq!(run.left, None, "the event's handler ran under a deadline");
    assert_eq!(stamps, vec![("emitted".to_owned(), None)], "the marked events the server's link delivered, with their `deadline-ms`");
    sleep::<B>(budget.settle).await;
    assert_eq!(probe.slow_runs().len(), 1, "the event reached its handler more than once: {:?}", probe.slow_runs());
    server.stop().await;
    client.close().await;
}

/// How many events the cancelled-execution scenario emits. A cancellation the client did not check
/// before the send would race the publish in its `select!`, decided at random, so one event would
/// not show it; eight leave a chance of 1 in 256 that a client sending at random sends none.
const CANCELLED_EMITS: usize = 8;

/// An event emitted `.within` an execution cancelled before the emit is awaited is not sent: each
/// of eight fails `Unavailable`, an event emitted outside the execution afterwards reaches its
/// handler, and two settle budgets on the server's link has delivered that one alone.
pub async fn cancelled_before_the_emit_is_not_sent<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    let settle = fixture.broker.budget().settle;
    let outcomes: Vec<Result<(), RpcError>> = fixture
        .client
        .execute(ExecOptions::new(), async |exec| {
            exec.cancel();
            let handle = exec.handle();
            let mut outcomes = Vec::new();
            for _ in 0..CANCELLED_EMITS {
                let event = fixture.rpc().emit(EVENT, &1u32).header(MARK, "cancelled").within(&handle);
                outcomes.push(within::<B, _>(Duration::from_secs(10), "the event's refusal", event.into_future()).await);
            }
            outcomes
        })
        .await;
    fixture.rpc().emit(EVENT, &2u32).header(MARK, "after").await.expect("an event outside the execution is published");
    let probe = fixture.server.probe.clone();
    let delivered = eventually::<B>(settle * 2, || tap.marks().iter().any(|mark| mark == "after")).await;
    assert!(delivered, "the event emitted outside the execution did not reach the server");
    sleep::<B>(settle * 2).await;
    assert_eq!(tap.marks(), vec!["after"], "the marked events the server's link delivered; the emits' outcomes: {outcomes:?}");
    assert_eq!((probe.events(), probe.last_event()), (1, Some(2)), "the events the handler ran");
    for outcome in outcomes {
        match outcome {
            Err(error) if error.kind() == ErrorKind::Unavailable => {}
            other => panic!("an event emitted inside a cancelled execution: expected `Unavailable`, got: {other:?}"),
        }
    }
    fixture.stop().await;
}

/// Cancellation stops at the send: an event emitted `.within` an execution that is cancelled once
/// the publish has returned reaches its handler, which runs to its end.
pub async fn cancelled_after_the_send_is_handled<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let budget = fixture.broker.budget();
    let runs = runs_of(budget);
    let sent = fixture
        .client
        .execute(ExecOptions::new(), async |exec| {
            let event = fixture.rpc().emit(SLOW_EVENT, &millis(runs)).within(&exec.handle());
            let sent = within::<B, _>(Duration::from_secs(10), "the event's publish", event.into_future()).await;
            exec.cancel();
            sent
        })
        .await;
    sent.expect("the event is published");
    let probe = fixture.server.probe.clone();
    let run = ended_once::<B>(|| probe.slow_runs(), budget.settle * 2 + runs, "the event emitted before the cancel").await;
    assert_eq!(run.ended, Some(SlowEnd::Completed), "the handler of an event sent before its emitter was cancelled");
    fixture.stop().await;
}
