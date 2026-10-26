//! Scenarios: drain.

use std::pin::pin;
use std::time::Duration;

use futures_util::future::{Either, select};
use futures_util::StreamExt;
use ulo::{Bound, FailureReason, Limit, ShutdownFailure, Signal, TypeName};
use ulo_rpc::{Capabilities, Rpc, RpcError};
use ulo_transport::ErrorKind;

use crate::Broker;
use crate::cases::app::{ADD, Add, EVENT, Fixture, HOLD, Sum, UNTIL_DRAIN, eventually, joined, miss_wait, server, streams, within};
use crate::rt::{sleep, spawn, yield_now};

/// How long the drain-window scenario waits between its calls, so a link that answers at once is
/// not flooded for the length of the drain.
const PACE: Duration = Duration::from_millis(10);

/// The event emitted during the drain on a link that declares `holds_unserved`.
const HELD_EVENT: u32 = 11;

/// What the server answers a call the drain's end abandoned, beside the kind `unavailable`, read
/// through the constant `ulo-transport` exports so the phrase has one source. The spent-cap
/// scenario compares it, since the client's own `unavailable` for a connection that closed carries
/// the same kind and no reason.
const SHUTDOWN_PHRASE: &str = ulo_transport::SHUTDOWN_MESSAGE;

/// How long the spent-cap scenario's call holds its handler: longer than the scenario runs.
const HELD_PAST_THE_CLOSE: u64 = 60_000;

/// The drain: a new call refused `unavailable`, an in-flight call finishing, a stream ending on
/// `draining()`. A link declaring `holds_unserved` answers the new call with the caller's own
/// `Timeout`, and an event emitted during the drain reaches the next instance.
pub async fn drain<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let capabilities = fixture.capabilities();
    let settle = fixture.broker.budget().settle;
    let held_for = u64::try_from((settle * 2 + Duration::from_millis(300)).as_millis()).unwrap_or(u64::MAX);

    let rpc = fixture.rpc().clone();
    let held = spawn::<B, _, _>(async move { rpc.request::<_, u64>(HOLD, &held_for).timeout(Duration::from_secs(30)).await });
    let mut ticking = if streams(&capabilities) {
        let mut ticks = fixture.rpc().stream::<_, u64>(UNTIL_DRAIN, &()).idle_timeout(Bound::After(Duration::from_secs(10))).await.expect("the stream opens");
        within::<B, _>(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields").expect("a tick arrives");
        Some(ticks)
    } else {
        None
    };
    // The held call reaches its handler before the drain starts.
    sleep::<B>(settle).await;

    let handle = fixture.server.handle.clone();
    let closing = spawn::<B, _, _>(async move { handle.close(Signal::new("conformance drain")).await });
    let draining = fixture.server.handle.clone();
    assert!(eventually::<B>(Duration::from_secs(5), || draining.is_draining()).await, "the server did not start draining");
    // The link's own close signal takes effect at the broker before the new call is sent.
    sleep::<B>(settle / 2).await;

    let refused = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(miss_wait(&fixture.broker)).await;
    match refused {
        Err(error) if error.kind() == ErrorKind::Unavailable => {}
        // A link without a miss signal declares the caller's own timeout as its answer to a call
        // nothing takes, as in the unhandled-pattern scenario; a link whose broker holds the call
        // for the next instance declares it too, and the held event below shows the hold.
        Err(error) if error.kind() == ErrorKind::Timeout && (!capabilities.miss_signal || capabilities.holds_unserved) => {}
        other => panic!("a call arriving during the drain is refused `Unavailable`, got: {other:?}"),
    }
    if capabilities.holds_unserved {
        fixture.rpc().emit(EVENT, &HELD_EVENT).await.expect("an event during the drain is published");
    }

    let held = joined(within::<B, _>(Duration::from_secs(30), "the in-flight call", held).await, "the held call's task");
    assert_eq!(held.expect("the in-flight call finishes during the drain"), held_for);
    if let Some(ticks) = ticking.take() {
        let rest: Vec<_> = within::<B, _>(Duration::from_secs(10), "the draining stream", ticks.collect()).await;
        assert!(rest.iter().all(Result::is_ok), "the stream ended on an error rather than cleanly: {rest:?}");
    }
    let _ = closing.await;
    if capabilities.holds_unserved {
        let budget = fixture.broker.budget();
        let next = server(&fixture.broker).await;
        let probe = next.probe.clone();
        let held = eventually::<B>(budget.boot + budget.settle, || probe.events() > 0).await;
        next.stop().await;
        assert!(held, "the event emitted during the drain did not reach the next instance, though the link declares `holds_unserved`");
        assert_eq!(probe.last_event(), Some(HELD_EVENT));
    }
    fixture.stop().await;
}

/// Calls sent from the drain's first moment until the server has closed, with no call held to
/// keep the drain open: each is refused `unavailable`, since a link's drain returns only once
/// nothing more is on its way to the server and the server answers everything that reached it. A
/// link declaring `confirms_drain(false)` cannot promise that, and may answer one with the
/// caller's own `Timeout`; a call sent once the server has closed is still `unavailable` there, which shows
/// the server stopped taking calls. A link without `miss_signal`, or declaring `holds_unserved`,
/// answers a call nothing takes with the caller's `Timeout`, as in the drain scenario.
pub async fn window<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let capabilities = fixture.capabilities();
    let wait = miss_wait(&fixture.broker);

    let handle = fixture.server.handle.clone();
    let mut closing = spawn::<B, _, _>(async move { handle.close(Signal::new("conformance drain window")).await });
    let draining = fixture.server.handle.clone();
    // Polled without a sleep, so the first call leaves as close to the drain's start as it can.
    let started = within::<B, _>(Duration::from_secs(5), "the server's drain", async {
        while !draining.is_draining() {
            yield_now().await;
        }
    });
    started.await;

    let mut during = Vec::new();
    within::<B, _>(Duration::from_secs(30), "the server's close", async {
        loop {
            let answer = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(wait).await;
            during.push(answer.map_err(|error| error.kind()));
            // `select` polls its left side first: a close that has ended ends the loop.
            if let Either::Left(_) = select(&mut closing, pin!(sleep::<B>(PACE))).await {
                return;
            }
        }
    })
    .await;
    let after = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 1, b: 1 }).timeout(wait).await;

    for (index, answer) in during.iter().enumerate() {
        let declared = answer == &Err(ErrorKind::Unavailable) || (answer == &Err(ErrorKind::Timeout) && times_out_during_drain(&capabilities));
        assert!(declared, "call {index} of {} sent during the drain was not refused `Unavailable`, got: {answer:?}", during.len());
    }
    assert_refused_after_close(&capabilities, after);
    fixture.stop().await;
}

/// Whether the caller's own `Timeout` is a declared answer for a call reaching a draining server.
fn times_out_during_drain(capabilities: &Capabilities) -> bool {
    !capabilities.miss_signal || capabilities.holds_unserved || !capabilities.confirms_drain
}

fn assert_refused_after_close(capabilities: &Capabilities, after: Result<Sum, RpcError>) {
    match after {
        Err(error) if error.kind() == ErrorKind::Unavailable => {}
        Err(error) if error.kind() == ErrorKind::Timeout && (!capabilities.miss_signal || capabilities.holds_unserved) => {}
        other => panic!("a call sent once the drained server had closed was not refused `Unavailable`, got: {other:?}"),
    }
}

/// A call still in its handler when the server closes under a `shutdown_timeout` already spent:
/// the drain's end cancels it, and the server's `close`, polled once before the cap drops it,
/// writes its `unavailable` with the shutdown phrase, which the caller reads rather than its own
/// timeout or a closed connection. The shutdown counts the call abandoned; a `close` that did
/// not end in that one poll is reported cut by the cap, at zero.
///
/// Stamped twice. On one thread nothing but `close`'s first poll runs the call's task before the
/// cap drops `close`. On the harness's runtime another worker may run it first, and the server's
/// app drops while the link's tasks may still hold the answer, which a link has to send.
pub async fn spent_cap<B: Broker>() {
    let fixture = Fixture::<B>::start_capped(Duration::ZERO).await;
    let rpc = fixture.rpc().clone();
    let held = spawn::<B, _, _>(async move { rpc.request::<_, u64>(HOLD, &HELD_PAST_THE_CLOSE).timeout(Duration::from_secs(30)).await });
    let probe = fixture.server.probe.clone();
    assert!(eventually::<B>(Duration::from_secs(10), || probe.holding() > 0).await, "the held call did not reach its handler");
    assert_eq!(probe.holding(), 1, "one call reached the holding handler");

    let closing = fixture.server.handle.close(Signal::new("conformance spent cap"));
    let abandoned = match within::<B, _>(Duration::from_secs(10), "the capped close", closing).await {
        Ok(shutdown) => shutdown.abandoned,
        Err(error) => {
            let [failure] = &*error.failures else { panic!("expected one failure, the RPC server's close, got: {error}") };
            match failure {
                ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } } => {
                    assert_eq!(*transport, TypeName::of::<Rpc>(), "the close the cap cut");
                    assert_eq!((*after, *limit), (Duration::ZERO, Limit::ShutdownCap), "the RPC server's close");
                }
                other => panic!("expected the RPC server's close cut by the cap, got: {other:?}"),
            }
            error.report.abandoned
        }
    };
    assert_eq!(abandoned, 1, "the held call is the one execution abandoned at the drain's end");

    let answer = joined(within::<B, _>(Duration::from_secs(30), "the held call's answer", held).await, "the held call's task");
    match answer {
        Err(error) => assert_eq!(
            (error.kind(), error.message(), error.reason()),
            (ErrorKind::Unavailable, SHUTDOWN_PHRASE, None),
            "the held call's answer: {error:?}"
        ),
        Ok(value) => panic!("a call abandoned at the drain's end was answered with its result, {value}"),
    }
    fixture.stop().await;
}
