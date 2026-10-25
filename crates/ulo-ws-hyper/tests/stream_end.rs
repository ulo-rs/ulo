//! `on_stream_end` reports the reply the dispatcher writes: a stream an interceptor puts in place of
//! the one it discarded reports its own end. A written stream reporting `Completed` and a discarded
//! one reporting nothing are the WebSocket conformance suite's `stream_end` scenarios
//! (`tests/conformance.rs`).
//!
//! Each handler registers a callback that sends the outcome on a channel the test holds the other
//! end of. The callback owns the channel's only sender, so the channel closes when the message's
//! execution ends: a test reads `None` for a stream that reported nothing, with no wait for a
//! report that might still come.
//!
//! A stream cancelled while it runs follows the pipeline's order: an item ready in the poll the
//! cancellation wakes is written, and no item is taken after that poll, so a stream whose items
//! are always ready ends at its next item. Each such stream writes the drain's reason itself, which
//! fixes the poll the cancellation is observed in; the connection is not draining and stays open.

mod support;

use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use serde_json::json;
use tokio::sync::{Notify, mpsc};
use ulo::{BoxError, CancelReason, ExecutionRef, Interceptor, Module, ModuleDef, ModuleIdentity, Next, StreamOutcome, injectable, routes};
use ulo_transport::{Classify, ErrorKind};
use ulo_ws::{Frame, Reply, Ws, WsCx, WsModule};

use support::{Running, hang_up, next_json, send_json};

const REPLACED: &str = "replaced";
const SELF_DRAINED: &str = "self_drained";
const SELF_CANCELLED: &str = "self_cancelled";
const GATED: &str = "gated";

/// How many items the six-item streams have; each would end after its last.
const SIX: u32 = 6;
/// The item `self_drained` cancels its execution on.
const CANCELLED_AT: u32 = 3;

/// What the six-item streams share with their tests.
#[derive(Default)]
struct Six {
    self_drained: AtomicUsize,
    self_cancelled: AtomicUsize,
    gated: AtomicUsize,
    /// The gated stream's second item waits here, and `waiting` is notified as it starts to.
    gate: Notify,
    waiting: Notify,
    /// The gated message's execution, for the test to cancel.
    exec: Mutex<Option<ExecutionRef>>,
}

fn six() -> &'static Six {
    static SIX_STATE: OnceLock<Six> = OnceLock::new();
    SIX_STATE.get_or_init(Six::default)
}

/// How long a test waits for the message's execution to end; it ends once the reply is written.
const PATIENCE: Duration = Duration::from_secs(5);

#[derive(Debug)]
struct Late;

impl fmt::Display for Late {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("late")
    }
}

impl Error for Late {}

impl Classify for Late {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

/// The sender for each event's message, taken by the handler that reports for it.
fn outcomes() -> MutexGuard<'static, HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>> {
    static OUTCOMES: OnceLock<Mutex<HashMap<&'static str, mpsc::UnboundedSender<StreamOutcome>>>> = OnceLock::new();
    OUTCOMES.get_or_init(Mutex::default).lock().unwrap_or_else(PoisonError::into_inner)
}

/// Registers the callback that reports `event`'s outcome, owning the channel's one sender.
fn report_for(event: &'static str, exec: &ExecutionRef) {
    let sender = outcomes().remove(event).expect("each event is sent once");
    exec.on_stream_end(move |outcome| {
        let _ = sender.send(outcome);
    });
}

fn ticks() -> impl Stream<Item = Result<u32, Late>> {
    stream::iter([Ok(1), Ok(2)])
}

/// Runs the message, drops the stream it answers and answers a stream of its own.
struct AnswerStream;

impl Interceptor<Ws> for AnswerStream {
    async fn intercept(&self, _cx: &WsCx, next: Next<'_, Ws>) -> Result<Reply, BoxError> {
        drop(next.run().await?);
        Ok(Reply::Many(Box::pin(stream::iter([Ok(Frame::text("1")), Ok(Frame::text("2"))]))))
    }
}

#[injectable]
struct Streams;

#[routes]
#[ulo_ws::gateway(path = "/streams", port = own)]
impl Streams {
    #[ulo_ws::message("replaced")]
    #[interceptors(value = AnswerStream)]
    fn replaced(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(REPLACED, &exec);
        ticks()
    }

    /// Six items, every one ready, the third cancelling the execution with the drain's reason as
    /// it is produced: the item and the cancellation are ready in one poll.
    #[ulo_ws::message("self_drained")]
    fn self_drained(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(SELF_DRAINED, &exec);
        stream::iter(1..=SIX).map(move |n| {
            six().self_drained.fetch_add(1, Ordering::SeqCst);
            if n == CANCELLED_AT {
                exec.cancel_with(CancelReason::Drain);
            }
            Ok(n)
        })
    }

    /// As `self_drained`, the third item cancelling the execution itself, `Explicit`.
    #[ulo_ws::message("self_cancelled")]
    fn self_cancelled(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(SELF_CANCELLED, &exec);
        stream::iter(1..=SIX).map(move |n| {
            six().self_cancelled.fetch_add(1, Ordering::SeqCst);
            if n == CANCELLED_AT {
                exec.cancel_with(CancelReason::Explicit);
            }
            Ok(n)
        })
    }

    /// Six items: the first ready, the second once the gate opens, the rest ready.
    #[ulo_ws::message("gated")]
    fn gated(&self, exec: ExecutionRef) -> impl Stream<Item = Result<u32, Late>> {
        report_for(GATED, &exec);
        *six().exec.lock().unwrap_or_else(PoisonError::into_inner) = Some(exec);
        stream::iter(1..=SIX).then(|n| async move {
            if n == 2 {
                six().waiting.notify_one();
                six().gate.notified().await;
            }
            six().gated.fetch_add(1, Ordering::SeqCst);
            Ok(n)
        })
    }

    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Streams>();
    }
}

/// Sends `event` with id `"m"` and reads `frames` frames of its reply; answers them and what its
/// execution reported once it ended, `None` for nothing.
async fn call(event: &'static str, frames: usize) -> (Vec<serde_json::Value>, Option<StreamOutcome>) {
    let (sender, mut reported) = mpsc::unbounded_channel();
    outcomes().insert(event, sender);
    let app = Running::start(Root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/streams", &[]).await;
    send_json(&mut socket, &json!({ "event": event, "id": "m" })).await;
    let mut written = Vec::new();
    for _ in 0..frames {
        written.push(next_json(&mut socket).await);
    }
    let ended = async {
        let first = reported.recv().await;
        if first.is_some() {
            assert_eq!(reported.recv().await, None, "{event}: the execution reported its stream's end twice");
        }
        first
    };
    let outcome = tokio::time::timeout(PATIENCE, ended).await.unwrap_or_else(|_| panic!("{event}: the execution did not end within {PATIENCE:?}"));
    hang_up(socket).await;
    app.stop().await;
    (written, outcome)
}

#[tokio::test]
async fn the_stream_replacing_a_discarded_one_reports_its_own_end() {
    let (frames, outcome) = call(REPLACED, 3).await;
    let expected = vec![json!({ "id": "m", "data": 1 }), json!({ "id": "m", "data": 2 }), json!({ "id": "m", "complete": true })];
    assert_eq!(frames, expected, "the interceptor's stream was not the reply");
    assert_eq!(outcome, Some(StreamOutcome::Completed), "the reply's own end was not what the execution reported");
}

/// The frames of a six-item stream cut after its `n`th item, read until its execution reports:
/// the items, the outcome, then a `ping` answered next on the same socket, which no further item
/// or `complete` precedes.
async fn cut_after(event: &'static str, socket: &mut support::Socket, reported: mpsc::UnboundedReceiver<StreamOutcome>, written: Vec<serde_json::Value>, n: u32) {
    cut_by(event, socket, reported, written, n, CancelReason::Drain).await;
}

/// [`cut_after`] for a stream cancelled with `reason`.
async fn cut_by(
    event: &'static str,
    socket: &mut support::Socket,
    mut reported: mpsc::UnboundedReceiver<StreamOutcome>,
    mut written: Vec<serde_json::Value>,
    n: u32,
    reason: CancelReason,
) {
    while written.len() < n as usize {
        written.push(next_json(socket).await);
    }
    let expected: Vec<_> = (1..=n).map(|item| json!({ "id": "m", "data": item })).collect();
    assert_eq!(written, expected, "{event}: the items written before the stream was cut");
    let outcome = tokio::time::timeout(PATIENCE, reported.recv()).await.unwrap_or_else(|_| panic!("{event}: the execution did not end within {PATIENCE:?}"));
    assert_eq!(outcome, Some(StreamOutcome::CutOff(Some(reason))), "{event}: the stream's end");
    send_json(socket, &json!({ "event": "ping", "id": "p" })).await;
    assert_eq!(next_json(socket).await, json!({ "id": "p", "data": "pong" }), "{event}: a frame of the cut stream followed its end");
}

#[tokio::test]
async fn a_stream_whose_items_are_always_ready_takes_no_item_past_the_poll_its_cancellation_is_observed_in() {
    let (sender, reported) = mpsc::unbounded_channel();
    outcomes().insert(SELF_DRAINED, sender);
    let app = Running::start(Root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/streams", &[]).await;
    send_json(&mut socket, &json!({ "event": SELF_DRAINED, "id": "m" })).await;

    cut_after(SELF_DRAINED, &mut socket, reported, Vec::new(), CANCELLED_AT).await;
    assert_eq!(six().self_drained.load(Ordering::SeqCst), CANCELLED_AT as usize, "an item was taken after the cancellation");
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn an_item_ready_in_the_poll_the_cancellation_wakes_is_written_and_none_after_it() {
    let (sender, reported) = mpsc::unbounded_channel();
    outcomes().insert(GATED, sender);
    let app = Running::start(Root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/streams", &[]).await;
    send_json(&mut socket, &json!({ "event": GATED, "id": "m" })).await;
    let first = next_json(&mut socket).await;
    tokio::time::timeout(PATIENCE, six().waiting.notified()).await.expect("the stream waits at its gate");
    // One thread: the pump's next poll finds both the cancellation and the item.
    let exec = six().exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
    exec.cancel_with(CancelReason::Drain);
    six().gate.notify_one();
    drop(exec);

    cut_after(GATED, &mut socket, reported, vec![first], 2).await;
    assert_eq!(six().gated.load(Ordering::SeqCst), 2, "an item was produced after the poll the cancellation woke");
    hang_up(socket).await;
    app.stop().await;
}

/// Any cancellation ends a WebSocket stream, not only the drain's: an always-ready stream that
/// cancels its own execution takes no item past that poll.
#[tokio::test]
async fn a_stream_whose_items_are_always_ready_ends_at_its_next_item_on_any_cancellation() {
    let (sender, reported) = mpsc::unbounded_channel();
    outcomes().insert(SELF_CANCELLED, sender);
    let app = Running::start(Root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    let mut socket = app.connect("/streams", &[]).await;
    send_json(&mut socket, &json!({ "event": SELF_CANCELLED, "id": "m" })).await;

    cut_by(SELF_CANCELLED, &mut socket, reported, Vec::new(), CANCELLED_AT, CancelReason::Explicit).await;
    assert_eq!(six().self_cancelled.load(Ordering::SeqCst), CANCELLED_AT as usize, "an item was taken after the cancellation");
    hang_up(socket).await;
    app.stop().await;
}
