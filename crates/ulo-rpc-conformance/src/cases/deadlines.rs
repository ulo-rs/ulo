//! Scenarios: deadlines.

use std::error::Error as _;
use std::future::{Future, IntoFuture};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use ulo::{AppHandle, Bound, BoundAddr, BoxError, CancelReason, ExecOptions, ExecutionRef, Runtime};
use ulo_rpc::{CallHeaders, Capabilities, Frame, Link, Outbound, Pattern, RpcClient, RpcError, RpcStream, ZeroTimeout};
use ulo_rpc::link::Inbound;
use ulo_transport::{Count, ErrorKind};

use crate::{Broker, Budget};
use crate::cases::app::{
    ADD, Add, COUNT, DEADLINE, DEADLINE_STREAM, DEADLINE_SUM, EVENT, Fixture, Mounts, PAUSE, STALL, SUM, Sum, TICKS, addresses,
    client_over, eventually, failed, refused_at_startup, streams, within,
};

/// `deadline-ms` fires `Deadline` and renders `timeout`: a call whose `req` carries 300 ms, written
/// by a [`Stamped`] link below a client waiting ten seconds, is answered the server's `timeout`
/// within five seconds, and its handler is ended `Deadline`.
pub async fn deadline_ms<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let link = Stamped { link: fixture.broker.client_link(&addresses(&[&fixture.server])), value: "300" };
    let client = client_over::<B, _>(link).await;
    let started = Instant::now();
    let outcome = client.rpc.request::<_, ()>(STALL, &()).timeout(Duration::from_secs(10)).await;
    failed(outcome, ErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(5), "the call was answered after {:?}, at the client's timeout rather than the server's", started.elapsed());
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(eventually::<B>(settle, || probe.stalled().is_some()).await, "the stalled handler was not ended");
    assert_eq!(probe.stalled(), Some(Some(CancelReason::Deadline)));
    client.close().await;
    fixture.stop().await;
}

/// The client's own timeout, as `Timeout`.
pub async fn client_timeout<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let started = Instant::now();
    let outcome = fixture.rpc().request::<_, ()>(STALL, &()).timeout(Duration::from_millis(300)).await;
    failed(outcome, ErrorKind::Timeout);
    assert!(started.elapsed() < Duration::from_secs(3), "the client's timeout took {:?}", started.elapsed());
    fixture.stop().await;
}

/// A call given a 300 ms timeout carries it as its deadline, and the server ends the call there.
/// The call goes through a client link that never sends a `cancel`, so the timeout's `cancel`
/// does not reach the server and only the deadline the call carried can end it.
pub async fn timeout_is_the_server_s_deadline<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let link = MutedCancel(fixture.broker.client_link(&addresses(&[&fixture.server])));
    let client = client_over::<B, _>(link).await;
    let started = Instant::now();
    let outcome = client.rpc.request::<_, ()>(STALL, &()).timeout(Duration::from_millis(300)).await;
    failed(outcome, ErrorKind::Timeout);
    let probe = fixture.server.probe.clone();
    let settle = fixture.broker.budget().settle;
    assert!(
        eventually::<B>(Duration::from_millis(300) + settle, || probe.stalled().is_some()).await,
        "the stalled handler was still running {:?} after a call with a 300 ms timeout began",
        started.elapsed()
    );
    assert_eq!(probe.stalled(), Some(Some(CancelReason::Deadline)), "what ended the stalled handler");
    client.close().await;
    fixture.stop().await;
}

/// The idle timeout the stream-deadline scenarios set, the unit their other bounds are written in:
/// the broker's settle budget, the time a frame takes to cross it, 500 ms at the default budget.
/// Each scenario asserts the order of its events, not their spacing, so its bounds grow with the
/// broker's latency and are otherwise short.
fn idle_of(budget: Budget) -> Duration {
    budget.settle
}

/// How long the scenarios below hold a stream silent between its two items: four idle timeouts,
/// two seconds at the default budget.
fn silence_of(budget: Budget) -> Duration {
    idle_of(budget) * 4
}

/// A stream's idle timeout bounds each silence and not the whole call: a stream read with an
/// idle timeout of the broker's settle budget, its ticks 20 ms apart, keeps yielding for three
/// idle timeouts.
pub async fn stream_idle_timeout_is_per_frame<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let idle = idle_of(fixture.broker.budget());
    let mut ticks = fixture.rpc().stream::<_, u64>(TICKS, &()).idle_timeout(Bound::After(idle)).await.expect("the stream opens");
    let opened = Instant::now();
    while opened.elapsed() < idle * 3 {
        let tick = within::<B, _>(Duration::from_secs(10), "a tick", ticks.next()).await.expect("the stream yields");
        if let Err(error) = tick {
            panic!("the stream failed {:?} after it opened, its idle timeout {idle:?}: {error:?}", opened.elapsed());
        }
    }
    drop(ticks);
    fixture.stop().await;
}

/// A stream given an idle timeout of the broker's settle budget, its server silent for four idle
/// timeouts after the first item, ends with `Timeout` once the idle timeout has passed and before
/// the silence ends.
pub async fn idle_timeout_ends_a_silent_stream<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let idle = idle_of(fixture.broker.budget());
    let silence_for = silence_of(fixture.broker.budget());
    let silence = silence_for.as_millis() as u64;
    let mut replies = fixture.rpc().stream::<_, u64>(PAUSE, &silence).idle_timeout(Bound::After(idle)).await.expect("the stream opens");
    let first = within::<B, _>(Duration::from_secs(10), "the first item", replies.next()).await.expect("the stream yields");
    assert_eq!(first.expect("the first item arrives"), 0);
    let silent = Instant::now();
    let ended = within::<B, _>(Duration::from_secs(10), "the idle timeout", replies.next()).await.expect("the stream yields its end");
    let waited = silent.elapsed();
    failed(ended, ErrorKind::Timeout);
    assert!(waited >= idle - Duration::from_millis(50), "the stream ended {waited:?} into the silence, before its idle timeout");
    assert!(waited < silence_for, "the stream ended {waited:?} into the silence, not at its {idle:?} idle timeout");
    assert!(replies.next().await.is_none(), "the stream yielded after its idle timeout");
    fixture.stop().await;
}

/// A stream given a deadline carries it as `deadline-ms`, and the server ends the stream there.
/// The deadline is the broker's settle budget and a fifth, 600 ms by default, so a tick crosses
/// the broker within it. The stream goes through a client link that never sends a `cancel`, so the
/// `cancel` the client sends when its own deadline passes does not reach the server.
pub async fn stream_deadline_is_the_server_s<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let settle = fixture.broker.budget().settle;
    let deadline = settle + settle / 5;
    let link = MutedCancel(fixture.broker.client_link(&addresses(&[&fixture.server])));
    let client = client_over::<B, _>(link).await;
    let started = Instant::now();
    let mut ticks = client.rpc.stream::<_, u64>(TICKS, &()).deadline(deadline).await.expect("the stream opens");
    let mut arrived = 0;
    let ended = loop {
        match within::<B, _>(Duration::from_secs(10), "the stream's deadline", ticks.next()).await {
            Some(Ok(_)) => arrived += 1,
            Some(Err(error)) => break error,
            None => panic!("the stream ended cleanly {:?} after it opened, its deadline {deadline:?}", started.elapsed()),
        }
    };
    assert_eq!(ended.kind(), ErrorKind::Timeout, "what ended the stream: {ended:?}");
    assert!(arrived > 0, "no tick arrived before the stream's deadline");
    drop(ticks);
    let probe = fixture.server.probe.clone();
    assert!(
        eventually::<B>(deadline + settle, || probe.stopped()).await,
        "the ticking stream was still running {:?} after a stream with a {deadline:?} deadline opened",
        started.elapsed()
    );
    assert_eq!(probe.cancelled(), Some(Some(CancelReason::Deadline)), "what ended the ticking stream");
    client.close().await;
    fixture.stop().await;
}

/// A stream given neither an idle timeout nor a deadline is bounded by neither, the client's
/// timeout included: on a client whose timeout is half the broker's settle budget, a stream silent
/// for four settle budgets between its two items yields both and ends cleanly.
pub async fn stream_with_neither_bound_runs_on<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let budget = fixture.broker.budget();
    let client = fixture.rpc().clone().timeout(Bound::After(idle_of(budget) / 2)).expect("a nonzero timeout");
    let started = Instant::now();
    let silence_for = silence_of(budget);
    let silence = silence_for.as_millis() as u64;
    let replies = client.stream::<_, u64>(PAUSE, &silence).await.expect("the stream opens");
    let items: Vec<_> = within::<B, _>(silence_for + Duration::from_secs(10), "the paused stream", replies.collect()).await;
    let items: Vec<u64> = items
        .into_iter()
        .map(|item| item.unwrap_or_else(|error| panic!("the stream failed {:?} after it opened: {error:?}", started.elapsed())))
        .collect();
    assert_eq!(items, vec![0, 1], "the paused stream's items");
    assert!(started.elapsed() >= silence_for, "the stream ended {:?} after it opened, before its silence", started.elapsed());
    fixture.stop().await;
}

/// The fixture's client with every stream's idle timeout set to `idle`.
fn idle_client<B: Broker>(fixture: &Fixture<B>, idle: Duration) -> RpcClient {
    fixture.rpc().clone().idle_timeout(Bound::After(idle)).expect("a nonzero idle timeout")
}

/// Opens a stream on [`PAUSE`] silent for `silence` between its two items, its idle timeout
/// `idle` on the call.
async fn paused<B: Broker>(client: &RpcClient, silence: Duration, idle: Bound) -> RpcStream<u64> {
    let millis = silence.as_millis() as u64;
    let opened = client.stream::<_, u64>(PAUSE, &millis).idle_timeout(idle);
    within::<B, _>(Duration::from_secs(10), "the stream's opening", opened.into_future()).await.expect("the stream opens")
}

/// Reads a stream on [`PAUSE`] to its end: both items, and an end no sooner than the silence
/// between them.
async fn runs_through_the_silence<B: Broker>(replies: RpcStream<u64>, silence: Duration, what: &str) {
    let started = Instant::now();
    let items: Vec<_> = within::<B, _>(silence + Duration::from_secs(10), "the paused stream", replies.collect()).await;
    let items: Vec<u64> = items
        .into_iter()
        .map(|item| item.unwrap_or_else(|error| panic!("{what}: the stream failed {:?} after it opened: {error:?}", started.elapsed())))
        .collect();
    assert_eq!(items, vec![0, 1], "{what}: the paused stream's items");
    assert!(started.elapsed() >= silence, "{what}: the stream ended {:?} after it opened, before its silence", started.elapsed());
}

/// Reads a stream on [`PAUSE`] past its first item into the silence, which must end it with
/// `Timeout` at `idle`, not before and not once the silence is over.
async fn ends_at<B: Broker>(mut replies: RpcStream<u64>, idle: Duration, silence: Duration, what: &str) {
    let first = within::<B, _>(Duration::from_secs(10), "the first item", replies.next()).await.expect("the stream yields");
    assert_eq!(first.expect("the first item arrives"), 0, "{what}: the first item");
    let silent = Instant::now();
    let ended = within::<B, _>(silence + Duration::from_secs(10), "the idle timeout", replies.next()).await.expect("the stream yields its end");
    let waited = silent.elapsed();
    failed(ended, ErrorKind::Timeout);
    assert!(waited >= idle - Duration::from_millis(50), "{what}: the stream ended {waited:?} into the silence, before its {idle:?} idle timeout");
    assert!(waited < silence, "{what}: the stream ended {waited:?} into the silence, not at its {idle:?} idle timeout");
}

/// On a client whose idle timeout is the broker's settle budget, a stream given
/// `Bound::Unbounded` on the call has none: silent for four settle budgets between its items, it
/// yields both and ends cleanly.
pub async fn unbounded_idle_timeout_outlasts_the_client_s<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let budget = fixture.broker.budget();
    let client = idle_client(&fixture, idle_of(budget));
    let replies = paused::<B>(&client, silence_of(budget), Bound::Unbounded).await;
    runs_through_the_silence::<B>(replies, silence_of(budget), "a stream at `Bound::Unbounded`").await;
    fixture.stop().await;
}

/// On a client whose idle timeout is the broker's settle budget, a stream given `Bound::Default`
/// on the call takes the client's: silent for four settle budgets after its first item, it ends
/// with `Timeout` at the client's idle timeout.
pub async fn default_idle_timeout_is_the_client_s<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let budget = fixture.broker.budget();
    let client = idle_client(&fixture, idle_of(budget));
    let replies = paused::<B>(&client, silence_of(budget), Bound::Default).await;
    ends_at::<B>(replies, idle_of(budget), silence_of(budget), "a stream at `Bound::Default`").await;
    fixture.stop().await;
}

/// On a client whose idle timeout outlasts the silence, eight settle budgets, a stream given the
/// settle budget on the call ends with `Timeout` there, four settle budgets before its second
/// item, which the client's idle timeout alone would let through.
pub async fn shorter_idle_timeout_replaces_the_client_s<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let budget = fixture.broker.budget();
    let client = idle_client(&fixture, silence_of(budget) * 2);
    let replies = paused::<B>(&client, silence_of(budget), Bound::After(idle_of(budget))).await;
    ends_at::<B>(replies, idle_of(budget), silence_of(budget), "a stream given a shorter idle timeout").await;
    fixture.stop().await;
}

/// A stream given an idle timeout of zero on the call fails when it is awaited, `BadRequest` with
/// the reason `zero_timeout` and a `ZeroTimeout` as its source, before its client connects or
/// sends a frame. A unary call through the same link afterwards shows the counts move.
pub async fn zero_idle_timeout_is_refused_before_any_frame<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let counts = Arc::new(Counts::default());
    let link = Counted { link: fixture.broker.client_link(&addresses(&[&fixture.server])), counts: Arc::clone(&counts) };
    let client = client_over::<B, _>(link).await;
    let refused = within::<B, _>(
        Duration::from_secs(10),
        "the refusal",
        client.rpc.stream::<_, u64>(PAUSE, &0u64).idle_timeout(Bound::After(Duration::ZERO)).into_future(),
    )
    .await;
    let refused = failed(refused.map(drop), ErrorKind::BadRequest);
    assert_eq!(refused.reason(), Some("zero_timeout"), "the refusal's reason: {refused:?}");
    assert!(
        refused.source().is_some_and(|source| source.downcast_ref::<ZeroTimeout>().is_some()),
        "the refusal's source is not a `ZeroTimeout`: {refused:?}"
    );
    assert_eq!(
        (counts.connects.load(Ordering::SeqCst), counts.frames.load(Ordering::SeqCst)),
        (0, 0),
        "the client's connects and frames sent before the refusal"
    );
    let sum = client.rpc.request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).timeout(Duration::from_secs(10)).await;
    assert_eq!(sum.expect("a unary call after the refusal is answered"), Sum { sum: 5 });
    assert!(
        counts.connects.load(Ordering::SeqCst) > 0 && counts.frames.load(Ordering::SeqCst) > 0,
        "the counts did not move for a call that reached the server"
    );
    client.close().await;
    fixture.stop().await;
}

/// On a client whose idle timeout is the broker's settle budget, a stream given
/// `Bound::After(Duration::MAX)` on the call, an end no `Instant` holds, neither panics nor ends
/// at the client's idle timeout: silent for four settle budgets, it yields both items and ends
/// cleanly.
pub async fn longest_idle_timeout_does_not_panic<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    if !streams(&fixture.capabilities()) {
        refused_at_startup(&fixture.broker, Mounts { streaming: true, binary: false }).await;
        return fixture.stop().await;
    }
    let budget = fixture.broker.budget();
    let client = idle_client(&fixture, idle_of(budget));
    let replies = paused::<B>(&client, silence_of(budget), Bound::After(Duration::MAX)).await;
    runs_through_the_silence::<B>(replies, silence_of(budget), "a stream at `Bound::After(Duration::MAX)`").await;
    fixture.stop().await;
}

/// The header each call in the zero-bound scenarios below carries, which the server's [`Tapped`]
/// link records for every opening frame it delivers.
pub(crate) const MARK: &str = "x-conformance-mark";

/// How many zero-bound calls each scenario below makes. A zero that is not refused times the call
/// out at once, and whether its `req` is sent first is a race the client's `select!` decides at
/// random, so one call that reaches no server would not show the refusal; eight leave a chance
/// of 1 in 256 that a client sending at random sends none of them.
const ZERO_CALLS: usize = 8;

/// Asserts `refused` is a zero bound's refusal: `BadRequest`, the reason `zero_timeout`, a
/// `ZeroTimeout` as its source.
fn zero_refused<T: std::fmt::Debug>(refused: Result<T, RpcError>, what: &str) {
    let refused = failed(refused, ErrorKind::BadRequest);
    assert_eq!(refused.reason(), Some("zero_timeout"), "{what}: the refusal's reason: {refused:?}");
    assert!(
        refused.source().is_some_and(|source| source.downcast_ref::<ZeroTimeout>().is_some()),
        "{what}: the refusal's source is not a `ZeroTimeout`: {refused:?}"
    );
}

/// F392: a call given `.timeout(Duration::ZERO)` fails when it is awaited, `BadRequest` with the
/// reason `zero_timeout` and a `ZeroTimeout` as its source, and its server sees no request: the
/// server's own link records the marked opening frames it delivers, and after eight refused calls
/// and a ninth with a ten-second timeout, answered, it has delivered the ninth alone. Every call
/// names one pattern over one connection, so a refused call's frame, had it been sent, would
/// have reached the server before the ninth's.
pub async fn zero_call_timeout_is_refused_before_the_server_sees_it<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    for _ in 0..ZERO_CALLS {
        let call = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "zero").timeout(Duration::ZERO);
        zero_refused(within::<B, _>(Duration::from_secs(10), "the refusal", call.into_future()).await, "`Call::timeout(Duration::ZERO)`");
    }
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "after").timeout(Duration::from_secs(10)).await;
    assert_eq!(sum.expect("a call with a ten-second timeout is answered"), Sum { sum: 5 });
    assert_eq!(tap.marks(), vec!["after"], "the marked calls the server received");
    fixture.stop().await;
}

/// F392: a stream given `.deadline(Duration::ZERO)` fails when it is awaited, `BadRequest` with
/// the reason `zero_timeout` and a `ZeroTimeout` as its source, and its server sees no request,
/// as in [`zero_call_timeout_is_refused_before_the_server_sees_it`]: the ninth stream, given a
/// ten-second deadline, is answered, its two items on a link carrying streamed replies and
/// `pattern_unhandled` on UDP, which mounts no streaming handler, and the server has delivered
/// it alone.
pub async fn zero_stream_deadline_is_refused_before_the_server_sees_it<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    for _ in 0..ZERO_CALLS {
        let call = fixture.rpc().stream::<_, u32>(COUNT, &2u32).header(MARK, "zero").deadline(Duration::ZERO);
        let refused = within::<B, _>(Duration::from_secs(10), "the refusal", call.into_future()).await;
        zero_refused(refused.map(drop), "`StreamCall::deadline(Duration::ZERO)`");
    }
    let call = fixture.rpc().stream::<_, u32>(COUNT, &2u32).header(MARK, "after").deadline(Duration::from_secs(10));
    let replies = within::<B, _>(Duration::from_secs(10), "the stream's opening", call.into_future()).await.expect("the stream opens");
    let items: Vec<_> = within::<B, _>(Duration::from_secs(10), "the stream", replies.collect()).await;
    if streams(&fixture.capabilities()) {
        let items: Vec<u32> = items.into_iter().map(|item| item.expect("the stream yields")).collect();
        assert_eq!(items, vec![1, 2], "the stream given a ten-second deadline");
    } else {
        match items.as_slice() {
            [Err(error)] if error.reason() == Some("pattern_unhandled") => {}
            other => panic!("a stream on a link carrying none was not answered `pattern_unhandled`: {other:?}"),
        }
    }
    assert_eq!(tap.marks(), vec!["after"], "the marked calls the server received");
    fixture.stop().await;
}

/// A call made `.within` an execution whose deadline has passed fails `Timeout` when it is
/// awaited, before it connects, and its server sees nothing. Two executions, each with a deadline
/// of now: a standalone one, which the app cancels at its deadline, and one opened as a transport
/// opens one, which nothing cancels, so its deadline alone refuses the calls. Inside each, a
/// request, a stream and an event, eight of each, each refused; then a request outside both,
/// answered, and the server has delivered that request alone. The event's frame names another
/// pattern, so a settle budget after the answer passes before the server's link is read.
pub async fn passed_deadline_is_refused_before_the_server_sees_it<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    let passed = crate::rt::runtime::<B>().now();
    let mut outcomes =
        fixture.client.execute(ExecOptions::new().deadline(passed), async |exec| past_deadline::<B>(&fixture, &exec.handle()).await).await;
    let opened = fixture.client.open(ExecOptions::new().deadline(passed));
    outcomes.extend(past_deadline::<B>(&fixture, &opened.handle()).await);
    assert!(!opened.is_cancelled(), "the opened execution was cancelled, so its deadline alone did not refuse the calls");
    drop(opened);
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "after").timeout(Duration::from_secs(10)).await;
    assert_eq!(sum.expect("a call outside the execution is answered"), Sum { sum: 5 });
    crate::rt::sleep::<B>(fixture.broker.budget().settle).await;
    assert_eq!(tap.marks(), vec!["after"], "the marked calls the server received; the calls' outcomes: {outcomes:?}");
    assert_eq!(fixture.server.probe.events(), 0, "an event sent past its execution's deadline reached its handler");
    for (what, outcome) in outcomes {
        match outcome {
            Err(error) if error.kind() == ErrorKind::Timeout => {}
            other => panic!("{what} made inside an execution whose deadline has passed: expected `Timeout`, got: {other:?}"),
        }
    }
    fixture.stop().await;
}

/// A request, a stream and an event made `.within` `exec`, eight of each, and how each ended.
async fn past_deadline<B: Broker>(fixture: &Fixture<B>, exec: &ExecutionRef) -> Vec<(&'static str, Result<(), RpcError>)> {
    let mut outcomes = Vec::new();
    for _ in 0..ZERO_CALLS {
        let call = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "passed").within(exec);
        outcomes.push(("a request", within::<B, _>(Duration::from_secs(10), "the request", call.into_future()).await.map(drop)));
        let stream = fixture.rpc().stream::<_, u32>(COUNT, &2u32).header(MARK, "passed").within(exec);
        outcomes.push(("a stream", within::<B, _>(Duration::from_secs(10), "the stream", stream.into_future()).await.map(drop)));
        let event = fixture.rpc().emit(EVENT, &1u32).header(MARK, "passed").within(exec);
        outcomes.push(("an event", within::<B, _>(Duration::from_secs(10), "the event", event.into_future()).await));
    }
    outcomes
}

/// A call made `.within` an execution already cancelled fails `Unavailable` when it is awaited,
/// before it connects, and its server sees nothing: a request and a stream, eight of each, each
/// refused, then a request outside the execution, answered, and the server has delivered that
/// request alone.
pub async fn cancelled_execution_s_call_is_refused_before_the_server_sees_it<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    let outcomes = fixture
        .client
        .execute(ExecOptions::new(), async |exec| {
            exec.cancel();
            let exec = exec.handle();
            let mut outcomes = Vec::new();
            for _ in 0..ZERO_CALLS {
                let call = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "cancelled").within(&exec);
                outcomes.push(("a request", within::<B, _>(Duration::from_secs(10), "the request", call.into_future()).await.map(drop)));
                let stream = fixture.rpc().stream::<_, u32>(COUNT, &2u32).header(MARK, "cancelled").within(&exec);
                outcomes.push(("a stream", within::<B, _>(Duration::from_secs(10), "the stream", stream.into_future()).await.map(drop)));
            }
            outcomes
        })
        .await;
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "after").timeout(Duration::from_secs(10)).await;
    assert_eq!(sum.expect("a call outside the execution is answered"), Sum { sum: 5 });
    assert_eq!(tap.marks(), vec!["after"], "the marked calls the server received; the calls' outcomes: {outcomes:?}");
    for (what, outcome) in outcomes {
        match outcome {
            Err(error) if error.kind() == ErrorKind::Unavailable => {}
            other => panic!("{what} made inside a cancelled execution: expected `Unavailable`, got: {other:?}"),
        }
    }
    fixture.stop().await;
}

/// The unit `deadline-ms` is written in, the most its truncation and its one-millisecond floor
/// move a deadline.
const DEADLINE_UNIT: Duration = Duration::from_millis(1);

/// F410: a call's `deadline-ms` is what is left of its caller's deadline when its frame is handed
/// to a connection already made, so the server's deadline is no later than the caller's, within
/// one unit, past the time the frame takes to reach it. A request, a stream and a streamed
/// request are made at once `.within` an execution, the first calls on a client whose link holds
/// each new connection for half the broker's settle budget before making it, the request and the
/// streamed request given a timeout longer than the execution has left, so the execution's
/// deadline is each call's own; the server's
/// handlers record the deadline they were given and the `deadline-ms` it came from, and the link
/// records when it was handed each call's frame. The frame's transit, from that handoff to the
/// server's reading of the header, is measured on the one clock the scenario's client and server
/// share: no value written before the frame is sent can account for it.
pub async fn measured_once_connected<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let budget = fixture.broker.budget();
    let held = budget.settle / 2;
    let link = SlowConnect::new(fixture.broker.client_link(&addresses(&[&fixture.server])), Hold::Waits(held));
    let handoffs = link.handoffs.clone();
    let client = client_over::<B, _>(link).await;
    let left = held + budget.boot;
    let deadline = crate::rt::runtime::<B>().now() + left;
    let opened = client.open(ExecOptions::new().deadline(deadline));
    let exec = opened.handle();
    let streaming = streams(&fixture.capabilities());
    let request = async {
        let call = client.rpc.request::<_, ()>(DEADLINE, &()).header(MARK, "request").timeout(left * 2).within(&exec);
        ("request", call.await)
    };
    let stream = async {
        let call = client.rpc.stream::<_, u32>(DEADLINE_STREAM, &()).header(MARK, "stream").within(&exec);
        let outcome = match call.await {
            Ok(replies) => replies.collect::<Vec<_>>().await.into_iter().collect::<Result<Vec<u32>, _>>().map(|items| assert_eq!(items, vec![1])),
            Err(error) => Err(error),
        };
        ("stream", outcome)
    };
    let streamed = async {
        let items = futures_util::stream::iter([1i64, 2]);
        let call = client.rpc.send_stream::<_, i64>(DEADLINE_SUM, items).header(MARK, "streamed").timeout(left * 2).within(&exec);
        ("streamed", call.await.map(|sum| assert_eq!(sum, 3)))
    };
    let calls = async {
        if streaming {
            let (request, stream, streamed) = futures_util::join!(request, stream, streamed);
            vec![request, stream, streamed]
        } else {
            vec![request.await]
        }
    };
    let outcomes = within::<B, _>(left + budget.settle, "the calls", calls).await;
    // A handle to an execution still open holds the client's app at its close.
    drop(exec);
    drop(opened);
    for (mark, outcome) in &outcomes {
        if let Err(error) = outcome {
            panic!("the {mark} made on a new connection failed: {error:?}");
        }
    }
    let seen = fixture.server.probe.seen();
    let handed = handoffs.all();
    let mut report = Vec::new();
    let mut late = false;
    for (mark, _) in &outcomes {
        let taken = seen.iter().find(|seen| seen.mark.as_deref() == Some(*mark)).unwrap_or_else(|| panic!("the server took no {mark}: {seen:?}"));
        let server = taken.deadline.unwrap_or_else(|| panic!("the server gave the {mark} no deadline: {taken:?}"));
        let millis: u64 = taken.header.as_deref().and_then(|value| value.parse().ok()).unwrap_or_else(|| panic!("the {mark} carried no readable `deadline-ms`: {taken:?}"));
        let read = server - Duration::from_millis(millis);
        let sent = handed.iter().find(|(handed, _)| handed == mark).map(|(_, at)| *at).unwrap_or_else(|| panic!("the link was handed no {mark}: {handed:?}"));
        let transit = read.saturating_duration_since(sent);
        late |= server > deadline + transit + DEADLINE_UNIT;
        report.push(format!(
            "the {mark}: `deadline-ms: {millis}`, the server's deadline {:+.3} ms from the caller's, its frame {:.3} ms in transit",
            signed_millis(server, deadline),
            transit.as_secs_f64() * 1e3
        ));
    }
    assert!(!late, "a server's deadline was later than its caller's by more than the frame's transit and one unit, its connect held {held:?}:\n{}", report.join("\n"));
    client.close().await;
    fixture.stop().await;
}

/// F410: a call whose connect outlasts its caller's deadline fails `Timeout` once the connect
/// returns, and its server sees nothing. For each shape the link carries, a request, a stream, a
/// streamed request and an event, a client whose link holds its connect twice as long as the
/// execution the call is made `.within` has left, a fifth of the broker's settle budget: once
/// with a connect that waits, which the call's own timer cuts short, and once with one that makes
/// its connection and then blocks its thread, so the connect returns in the poll that finds the
/// deadline passed, before the call's timer can be polled again. Then a
/// request on a connection already made, answered, and the server has delivered that request
/// alone, no event handled.
///
/// Only the blocking half is evidence that a frame is stamped and refused once the connection is
/// in hand. The waiting half pins behaviour that predates that check: the call's timer ends a
/// connect that waits at the deadline, so it passed against the client before the check existed.
/// The block follows the connection rather than preceding it: a block ahead of a connect that still
/// awaits ends in a poll that returns pending, and the call's timer, armed for what is left of the
/// bound, answers at the next poll whether or not the check exists.
pub async fn connect_past_the_deadline_is_refused_before_the_server_sees_it<B: Broker>() {
    let tap = Tap::default();
    let fixture = Fixture::<B>::start_over(Count::Default, |broker| Tapped { link: broker.link(), tap: tap.clone() }).await;
    let budget = fixture.broker.budget();
    let left = budget.settle / 5;
    let held = left * 2;
    let streaming = streams(&fixture.capabilities());
    let mut outcomes = Vec::new();
    // `Hold::Waits` is the pin, `Hold::BlocksConnected` the evidence; see the doc above.
    for hold in [Hold::Waits(held), Hold::BlocksConnected(held)] {
        for shape in ["a request", "a stream", "a streamed request", "an event"] {
            if !streaming && matches!(shape, "a stream" | "a streamed request") {
                continue;
            }
            let link = SlowConnect::new(fixture.broker.client_link(&addresses(&[&fixture.server])), hold);
            let client = client_over::<B, _>(link).await;
            let opened = client.open(ExecOptions::new().deadline(crate::rt::runtime::<B>().now() + left));
            let exec = opened.handle();
            let call = async {
                match shape {
                    "a request" => client.rpc.request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "slow").within(&exec).await.map(drop),
                    "a stream" => client.rpc.stream::<_, u32>(COUNT, &2u32).header(MARK, "slow").within(&exec).await.map(drop),
                    "a streamed request" => {
                        let items = futures_util::stream::iter([1i64, 2]);
                        client.rpc.send_stream::<_, i64>(SUM, items).header(MARK, "slow").within(&exec).await.map(drop)
                    }
                    _ => client.rpc.emit(EVENT, &1u32).header(MARK, "slow").within(&exec).await,
                }
            };
            let outcome = within::<B, _>(held + budget.boot, "the call", call).await;
            outcomes.push((hold, shape, outcome));
            // A handle to an execution still open holds the client's app at its close.
            drop(exec);
            drop(opened);
            client.close().await;
        }
    }
    let sum = fixture.rpc().request::<_, Sum>(ADD, &Add { a: 2, b: 3 }).header(MARK, "after").timeout(Duration::from_secs(10)).await;
    assert_eq!(sum.expect("a call on a connection already made is answered"), Sum { sum: 5 });
    crate::rt::sleep::<B>(budget.settle).await;
    assert_eq!(tap.marks(), vec!["after"], "the marked calls the server received; the calls' outcomes: {outcomes:?}");
    assert_eq!(fixture.server.probe.events(), 0, "an event whose connect outlasted its deadline reached its handler: {outcomes:?}");
    for (hold, shape, outcome) in outcomes {
        match outcome {
            Err(error) if error.kind() == ErrorKind::Timeout => {}
            other => panic!("{shape} whose connect {hold:?} past its deadline: expected `Timeout`, got: {other:?}"),
        }
    }
    fixture.stop().await;
}

/// How many calls [`blocked_connect_s_timer_ends_with_its_bound`] makes. Its timer starts late
/// only when the call's `select!` polls the work before the timer, an order it shuffles at random,
/// so half the time; sixteen leave a chance of one in 65,536 that a timer started late is never
/// seen.
const BLOCKED_CALLS: usize = 16;

/// The `deadline-ms` [`blocked_connect_s_timer_ends_with_its_bound`]'s server is given, ten
/// seconds, past every bound the scenario sets: the client's `cancel` stops the handler.
const UNANSWERED: &str = "10000";

/// F411: a call's timer ends where its bound does, whatever blocked the call's first poll. Sixteen
/// requests, each the first call of a fresh client whose link blocks its thread in the connect for
/// half the call's timeout, two fifths of the broker's settle budget, to a handler that stalls. A
/// [`Stamped`] link below the client writes the server a ten-second deadline, `UNANSWERED`, so the
/// server's own `timeout` does not answer the call at its bound and only the client's timer can.
/// Each fails `Timeout` no later than its timeout, the later of the bound's end and the block's,
/// and a tenth of the settle budget, room for the poll that finds the timer due, after it was
/// awaited. A timer counted from its own first poll, which the block delays, fires a full timeout
/// after the block ends, half a timeout past the bound's end. The block leaves the link's own
/// connect half the timeout to finish in, so the frame is sent and the timer alone ends the call:
/// a connect returning past the bound is refused there instead, whatever the timer does.
pub async fn blocked_connect_s_timer_ends_with_its_bound<B: Broker>() {
    let fixture = Fixture::<B>::start().await;
    let budget = fixture.broker.budget();
    let timeout = budget.settle * 2 / 5;
    let blocked = timeout / 2;
    let margin = budget.settle / 10;
    let mut outcomes = Vec::new();
    for _ in 0..BLOCKED_CALLS {
        let stamped = Stamped { link: fixture.broker.client_link(&addresses(&[&fixture.server])), value: UNANSWERED };
        let client = client_over::<B, _>(SlowConnect::new(stamped, Hold::Blocks(blocked))).await;
        let started = Instant::now();
        let call = client.rpc.request::<_, ()>(STALL, &()).timeout(timeout).into_future();
        let outcome = within::<B, _>(timeout + blocked + budget.boot, "the call", call).await;
        outcomes.push((started.elapsed(), outcome));
        client.close().await;
    }
    let report: Vec<String> = outcomes
        .iter()
        .map(|(took, outcome)| format!("{:.1} ms, {:?}", took.as_secs_f64() * 1e3, outcome.as_ref().map_err(RpcError::kind)))
        .collect();
    let latest = timeout.max(blocked) + margin;
    for (took, outcome) in &outcomes {
        assert!(
            matches!(outcome, Err(error) if error.kind() == ErrorKind::Timeout) && *took <= latest,
            "a call given {timeout:?} over a connect blocking {blocked:?} was answered later than {latest:?} or not `Timeout`; every call:\n{}",
            report.join("\n")
        );
    }
    fixture.stop().await;
}

/// `a - b` in milliseconds, negative when `a` is the earlier.
fn signed_millis(a: Instant, b: Instant) -> f64 {
    match a.checked_duration_since(b) {
        Some(later) => later.as_secs_f64() * 1e3,
        None => -(b.duration_since(a).as_secs_f64() * 1e3),
    }
}

/// How a [`SlowConnect`] link holds each connect.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// Waits on the client's runtime before making the connection, as a connect waiting on its
    /// peer does.
    Waits(Duration),
    /// Blocks the thread polling the connect before making the connection, as a connect that does
    /// blocking work first does: the connect's first poll lasts the block.
    Blocks(Duration),
    /// Makes the connection, then blocks the thread polling the connect, as a connect finishing with
    /// blocking work does: the connect returns in the poll that ends the block, the call's next step
    /// with it.
    BlocksConnected(Duration),
}

/// When a [`SlowConnect`] link was handed each opening frame or event carrying [`MARK`], with the
/// mark, on the client's clock.
#[derive(Clone, Default)]
struct Handoffs(Arc<Mutex<Vec<(String, Instant)>>>);

impl Handoffs {
    fn all(&self) -> Vec<(String, Instant)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A client link holding each connect for its [`Hold`] before making it, as a slow network or a
/// handshake does, and recording its [`Handoffs`].
struct SlowConnect<L> {
    link: L,
    hold: Hold,
    handoffs: Handoffs,
}

impl<L> SlowConnect<L> {
    fn new(link: L, hold: Hold) -> Self {
        SlowConnect { link, hold, handoffs: Handoffs::default() }
    }
}

impl<L: Link> Link for SlowConnect<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.link.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.link.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.link.max_inflight(calls);
    }

    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        self.link.listen(patterns)
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        match self.hold {
            Hold::Waits(held) => runtime.sleep(held).await,
            Hold::Blocks(held) => std::thread::sleep(held),
            Hold::BlocksConnected(_) => {}
        }
        let Outbound { send, replies } = self.link.connect(runtime).await?;
        if let Hold::BlocksConnected(held) = self.hold {
            std::thread::sleep(held);
        }
        let (handoffs, clock) = (self.handoffs.clone(), Arc::clone(runtime));
        Ok(Outbound {
            send: Box::new(move |pattern, frame, reply_to| {
                let headers = match &frame {
                    Frame::Req { headers, .. } | Frame::Open { headers, .. } | Frame::Evt { headers, .. } => headers.get(MARK),
                    _ => None,
                };
                if let Some(mark) = headers {
                    handoffs.0.lock().unwrap_or_else(PoisonError::into_inner).push((mark.to_owned(), clock.now()));
                }
                send(pattern, frame, reply_to)
            }),
            replies,
        })
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.link.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}

/// The value of [`MARK`] on each opening frame a [`Tapped`] link delivered, in arrival order, with
/// the frame's `deadline-ms`.
#[derive(Clone, Default)]
pub(crate) struct Tap(Arc<Mutex<Vec<Stamp>>>);

/// A delivered mark, and the `deadline-ms` its frame carried, `None` for none.
pub(crate) type Stamp = (String, Option<String>);

impl Tap {
    pub(crate) fn marks(&self) -> Vec<String> {
        self.stamps().into_iter().map(|(mark, _)| mark).collect()
    }

    pub(crate) fn stamps(&self) -> Vec<Stamp> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// A server link recording, in its [`Tap`], every opening frame carrying [`MARK`] that it hands
/// the server, a `req`, an `open` or an `evt`, and the frame's `deadline-ms`.
pub(crate) struct Tapped<L> {
    pub(crate) link: L,
    pub(crate) tap: Tap,
}

impl<L: Link> Link for Tapped<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.link.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.link.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.link.max_inflight(calls);
    }

    async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        let inbound = self.link.listen(patterns).await?;
        let tap = self.tap.clone();
        Ok(Box::pin(inbound.inspect(move |delivery| {
            let headers = match &delivery.frame {
                Frame::Req { headers, .. } | Frame::Open { headers, .. } | Frame::Evt { headers, .. } => headers,
                _ => return,
            };
            if let Some(mark) = headers.get(MARK) {
                let deadline = headers.get("deadline-ms").map(str::to_owned);
                tap.0.lock().unwrap_or_else(PoisonError::into_inner).push((mark.to_owned(), deadline));
            }
        })))
    }

    fn connect(&self, runtime: &Arc<dyn Runtime>) -> impl Future<Output = Result<Outbound, BoxError>> + Send {
        self.link.connect(runtime)
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.link.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}

/// How many times a [`Counted`] link connected and sent a frame.
#[derive(Default)]
struct Counts {
    connects: AtomicUsize,
    frames: AtomicUsize,
}

/// A client link counting its connects and the frames it sends.
struct Counted<L> {
    link: L,
    counts: Arc<Counts>,
}

impl<L: Link> Link for Counted<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.link.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.link.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.link.max_inflight(calls);
    }

    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        self.link.listen(patterns)
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        self.counts.connects.fetch_add(1, Ordering::SeqCst);
        let Outbound { send, replies } = self.link.connect(runtime).await?;
        let counts = Arc::clone(&self.counts);
        Ok(Outbound {
            send: Box::new(move |pattern, frame, reply_to| {
                counts.frames.fetch_add(1, Ordering::SeqCst);
                send(pattern, frame, reply_to)
            }),
            replies,
        })
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.link.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}

/// A client link that sends every frame but a `cancel`.
struct MutedCancel<L>(L);

impl<L: Link> Link for MutedCancel<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.0.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.0.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.0.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.0.max_inflight(calls);
    }

    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        self.0.listen(patterns)
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let Outbound { send, replies } = self.0.connect(runtime).await?;
        Ok(Outbound {
            send: Box::new(move |pattern, frame, reply_to| match frame {
                Frame::Cancel { .. } => Box::pin(async { Ok(()) }),
                frame => send(pattern, frame, reply_to),
            }),
            replies,
        })
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.0.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.0.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.0.bound()
    }
}

/// A client link writing `deadline-ms: value` on every `req`, `open` and `evt` it sends, in place
/// of any the client stamped. The client refuses a `deadline-ms` its caller sets, so a value it
/// would not send, a deadline shorter than the client's own wait or one the server cannot read,
/// reaches a server only from below the client. Public for a link crate's own tests of the same.
pub struct Stamped<L> {
    pub link: L,
    pub value: &'static str,
}

impl<L: Link> Link for Stamped<L> {
    const NAME: &'static str = L::NAME;

    fn capabilities(&self) -> Capabilities {
        self.link.capabilities()
    }

    fn prepare(&mut self, app: &AppHandle) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.prepare(app)
    }

    fn usable(&self) -> Result<(), BoxError> {
        self.link.usable()
    }

    fn max_inflight(&mut self, calls: Count) {
        self.link.max_inflight(calls);
    }

    fn listen(&self, patterns: &[Pattern]) -> impl Future<Output = Result<Inbound, BoxError>> + Send {
        self.link.listen(patterns)
    }

    async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let Outbound { send, replies } = self.link.connect(runtime).await?;
        let value = self.value;
        Ok(Outbound {
            send: Box::new(move |on, frame, reply_to| {
                let frame = match frame {
                    Frame::Req { id, pattern, headers, data } => Frame::Req { id, pattern, headers: stamped(&headers, value), data },
                    Frame::Open { id, pattern, headers } => Frame::Open { id, pattern, headers: stamped(&headers, value) },
                    Frame::Evt { pattern, headers, data } => Frame::Evt { pattern, headers: stamped(&headers, value), data },
                    other => other,
                };
                send(on, frame, reply_to)
            }),
            replies,
        })
    }

    fn drain(&self) -> impl Future<Output = ()> + Send {
        self.link.drain()
    }

    fn close(&self) -> impl Future<Output = Result<(), BoxError>> + Send {
        self.link.close()
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.link.bound()
    }
}

/// `headers` with every `deadline-ms` replaced by one carrying `value`.
fn stamped(headers: &CallHeaders, value: &str) -> CallHeaders {
    let mut out = CallHeaders::new();
    for (name, kept) in headers.iter().filter(|(name, _)| *name != "deadline-ms") {
        out.insert(name, kept);
    }
    out.insert("deadline-ms", value);
    out
}
