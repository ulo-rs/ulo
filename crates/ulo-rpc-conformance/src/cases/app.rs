//! The application every scenario runs, the client beside it, and the waits the scenarios share.
//!
//! The handlers are split across three controllers so a link refusing a shape or a payload kind
//! in `prepare` still runs every scenario it carries: the streaming controller is mounted only
//! where the link's `shapes` carry every streamed shape, the binary one only where it declares
//! `binary`.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::future::select;
use futures_util::stream::BoxStream;
use futures_util::{Stream, StreamExt, stream};
use serde::{Deserialize, Serialize};
use ulo::app::{Bound as Serving, Connected};
use ulo::{
    App, AppHandle, BoundAddr, BoxError, Bound, CancelReason, Dep, ErrorHandler, ExecOptions, Execution, ExecutionRef, Guard,
    Module, ModuleDef, ModuleIdentity, ModuleRef, Shape, Shutdown, ShutdownError, Signal, StartupError, TaskEnd, Timer,
    ValueHandle, injectable, routes,
};
use ulo_rpc::{
    CallHeaders, Capabilities, Data, Inbound, Link, Payload, Reply, Rpc, RpcClient, RpcClientModule, RpcCx, RpcError,
};
use ulo_transport::{CallError, Classify, Count, ErrorKind};

use crate::cases::deadlines::MARK;
use crate::rt::{sleep, spawn, timeout};
use crate::{Broker, Harness, report};

pub(crate) const ADD: &str = "conformance.add";
pub(crate) const ECHO: &str = "conformance.echo";
pub(crate) const CONFLICT: &str = "conformance.conflict";
pub(crate) const GUARDED: &str = "conformance.guarded";
pub(crate) const BOOM: &str = "conformance.boom";
pub(crate) const HEADERS: &str = "conformance.headers";
pub(crate) const EVENT: &str = "conformance.event";
pub(crate) const TALLY: &str = "conformance.tally";
pub(crate) const SLOW_EVENT: &str = "conformance.slow_event";
pub(crate) const STALL: &str = "conformance.stall";
pub(crate) const HOLD: &str = "conformance.hold";
pub(crate) const NEVER: &str = "conformance.never";
pub(crate) const COUNT: &str = "conformance.count";
pub(crate) const SUM: &str = "conformance.sum";
pub(crate) const COLLECT: &str = "conformance.collect";
pub(crate) const DOUBLE: &str = "conformance.double";
pub(crate) const TICKS: &str = "conformance.ticks";
pub(crate) const PAUSE: &str = "conformance.pause";
pub(crate) const UNTIL_DRAIN: &str = "conformance.until_drain";
pub(crate) const BYTES: &str = "conformance.bytes";
pub(crate) const NOBODY: &str = "conformance.nobody";
pub(crate) const NOBODY_EVENT: &str = "conformance.nobody.event";
pub(crate) const SUBSTITUTED: &str = "conformance.substituted";
pub(crate) const SUBSTITUTED_STREAM: &str = "conformance.substituted_stream";
pub(crate) const UNENCODABLE: &str = "conformance.unencodable";
pub(crate) const UNENCODABLE_STREAM: &str = "conformance.unencodable_stream";
pub(crate) const CONTEXT: &str = "conformance.context";
pub(crate) const BOUNDED: &str = "conformance.bounded";
pub(crate) const BOUNDED_TOO: &str = "conformance.bounded_too";
pub(crate) const DEADLINE: &str = "conformance.deadline";
pub(crate) const DEADLINE_STREAM: &str = "conformance.deadline_stream";
pub(crate) const DEADLINE_SUM: &str = "conformance.deadline_sum";

/// A payload no codec reads: not UTF-8, so no JSON text, and a lone CBOR break code, so no CBOR
/// item. A reply carrying it fails the link's frame encoding.
const GARBLED: &[u8] = &[0xff];

/// What [`Substitute`] answers a call's `Refusal::Missing` with.
pub(crate) const SUBSTITUTE: Sum = Sum { sum: 42 };

/// The items [`SubstituteStream`] answers a stream's `Refusal::Missing` with.
pub(crate) const SUBSTITUTE_ITEMS: [u32; 3] = [7, 8, 9];

/// The header the headers scenario sends and reads back.
pub(crate) const HEADER: &str = "x-conformance";

/// The server's drain window: longer than any call a scenario holds in flight across it.
const DRAIN: Duration = Duration::from_secs(5);

/// The client module's timeout, which a request's own `.timeout(..)` replaces; a stream has none.
const CALL_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Add {
    pub(crate) a: i64,
    pub(crate) b: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Sum {
    pub(crate) sum: i64,
}

/// What the server's handlers record for a scenario to read: one per server instance.
#[derive(Clone, Default)]
pub(crate) struct Probe(Arc<ProbeState>);

#[derive(Default)]
pub(crate) struct ProbeState {
    events: AtomicUsize,
    last_event: Mutex<Option<u32>>,
    tallies: AtomicUsize,
    /// The ticking stream's execution's reason, written when the stream is dropped.
    cancelled: Mutex<Option<Option<CancelReason>>>,
    stopped: AtomicBool,
    /// How many calls have reached the handler that never answers.
    unanswered: AtomicUsize,
    /// The stalled call's execution's reason, written when its handler's future is dropped or
    /// returns.
    stalled: Mutex<Option<Option<CancelReason>>>,
    /// How many calls have reached the holding handler.
    holding: AtomicUsize,
    /// How many `add` calls have reached their handler, and how many of their executions have
    /// ended, counted by [`AddEnd`]'s drop.
    adds_begun: AtomicUsize,
    adds_ended: AtomicUsize,
    /// When the held call's handler returned its answer, which the link then publishes.
    held_answered: Mutex<Option<Instant>>,
    /// The bounded handlers' calls running now, and the most that ran at once.
    bounded: AtomicUsize,
    bounded_peak: AtomicUsize,
    /// Each run of the slow event's handler, in the order they began.
    slow: Mutex<Vec<SlowRun>>,
    /// Each call the deadline handlers took, in the order they took them.
    seen: Mutex<Vec<Seen>>,
}

/// A call one of the deadline handlers took: its [`MARK`], the deadline its server computed for
/// it, and the `deadline-ms` the server computed that from, `None` for none.
#[derive(Clone, Debug)]
pub(crate) struct Seen {
    pub(crate) mark: Option<String>,
    pub(crate) deadline: Option<Instant>,
    pub(crate) header: Option<String>,
}

/// One run of the slow event's handler: when it began, the time its execution's deadline left it
/// then, `None` for an execution without one, and how it ended, `None` while it runs.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SlowRun {
    pub(crate) began: Instant,
    pub(crate) left: Option<Duration>,
    pub(crate) ended: Option<SlowEnd>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlowEnd {
    Completed,
    /// Its future was dropped before it completed, with its execution's cancel reason.
    Dropped(Option<CancelReason>),
}

/// Ends the slow event's run it was made for when dropped: `Completed` once the handler marked it
/// so, `Dropped` otherwise.
struct SlowGuard {
    exec: ExecutionRef,
    probe: Probe,
    run: usize,
    completed: bool,
}

impl Drop for SlowGuard {
    fn drop(&mut self) {
        let ended = if self.completed { SlowEnd::Completed } else { SlowEnd::Dropped(self.exec.cancel_reason()) };
        if let Some(run) = lock(&self.probe.0.slow).get_mut(self.run) {
            run.ended = Some(ended);
        }
    }
}

impl Probe {
    pub(crate) fn events(&self) -> usize {
        self.0.events.load(Ordering::SeqCst)
    }

    pub(crate) fn last_event(&self) -> Option<u32> {
        *lock(&self.0.last_event)
    }

    pub(crate) fn tallies(&self) -> usize {
        self.0.tallies.load(Ordering::SeqCst)
    }

    pub(crate) fn cancelled(&self) -> Option<Option<CancelReason>> {
        *lock(&self.0.cancelled)
    }

    pub(crate) fn stopped(&self) -> bool {
        self.0.stopped.load(Ordering::SeqCst)
    }

    pub(crate) fn unanswered(&self) -> usize {
        self.0.unanswered.load(Ordering::SeqCst)
    }

    pub(crate) fn stalled(&self) -> Option<Option<CancelReason>> {
        *lock(&self.0.stalled)
    }

    pub(crate) fn held_answered(&self) -> Option<Instant> {
        *lock(&self.0.held_answered)
    }

    /// Whether every `add` call that reached its handler has had its execution end, and one did.
    pub(crate) fn adds_settled(&self) -> bool {
        // `ended` is read first: a call beginning between the two reads only makes the pair unequal.
        let ended = self.0.adds_ended.load(Ordering::SeqCst);
        let begun = self.0.adds_begun.load(Ordering::SeqCst);
        begun > 0 && begun == ended
    }

    /// How many calls have reached the holding handler.
    pub(crate) fn holding(&self) -> usize {
        self.0.holding.load(Ordering::SeqCst)
    }

    /// Every run of the slow event's handler so far.
    pub(crate) fn slow_runs(&self) -> Vec<SlowRun> {
        lock(&self.0.slow).clone()
    }

    /// Every call the deadline handlers took so far.
    pub(crate) fn seen(&self) -> Vec<Seen> {
        lock(&self.0.seen).clone()
    }

    fn see(&self, exec: &ExecutionRef, headers: &CallHeaders) {
        let seen = Seen {
            mark: headers.get(MARK).map(str::to_owned),
            deadline: exec.deadline(),
            header: headers.get("deadline-ms").map(str::to_owned),
        };
        lock(&self.0.seen).push(seen);
    }

    /// The most calls the two bounded handlers ran at once.
    pub(crate) fn bounded_peak(&self) -> usize {
        self.0.bounded_peak.load(Ordering::SeqCst)
    }

    /// Counts one bounded call running for `millis` on `timer`.
    async fn bounded(&self, timer: &dyn Timer, millis: u64) -> u64 {
        let now = self.0.bounded.fetch_add(1, Ordering::SeqCst) + 1;
        self.0.bounded_peak.fetch_max(now, Ordering::SeqCst);
        timer.sleep(Duration::from_millis(millis)).await;
        self.0.bounded.fetch_sub(1, Ordering::SeqCst);
        millis
    }
}

/// Records its execution's cancel reason when dropped: a handler's future dropped at a deadline,
/// or a reply stream dropped at a `cancel`, reports what ended it.
struct Recorder {
    exec: ExecutionRef,
    probe: Probe,
    stream: bool,
}

impl Drop for Recorder {
    fn drop(&mut self) {
        let reason = self.exec.cancel_reason();
        if self.stream {
            *lock(&self.probe.0.cancelled) = Some(reason);
            self.probe.0.stopped.store(true, Ordering::SeqCst);
        } else {
            *lock(&self.probe.0.stalled) = Some(reason);
        }
    }
}

/// Built for the execution of each `add` call and dropped inside `Execution::drop`, after the
/// execution is marked ended, which is the exact moment the shutdown stops counting the call
/// abandoned.
#[injectable(execution)]
struct AddEnd {
    probe: Dep<Probe>,
}

impl Drop for AddEnd {
    fn drop(&mut self) {
        self.probe.0.adds_ended.fetch_add(1, Ordering::SeqCst);
    }
}

/// A handler's own failure: `Conflict` for the domain-error scenario, `BadRequest` for a
/// streamed item that does not decode, `NotFound` for the one an error handler answers with a
/// value of its own.
#[derive(Debug)]
pub(crate) enum Refusal {
    Conflict,
    BadItem,
    Missing,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Conflict => f.write_str("the conformance resource already exists"),
            Refusal::BadItem => f.write_str("a streamed item did not decode"),
            Refusal::Missing => f.write_str("the conformance resource does not exist"),
        }
    }
}

impl Error for Refusal {}

impl Classify for Refusal {
    fn classify(&self) -> ErrorKind {
        match self {
            Refusal::Conflict => ErrorKind::Conflict,
            Refusal::BadItem => ErrorKind::BadRequest,
            Refusal::Missing => ErrorKind::NotFound,
        }
    }
}

/// Whether `err` is a handler's `Refusal::Missing`.
fn missing(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().and_then(|call| call.source_as::<Refusal>()).is_some_and(|refusal| matches!(refusal, Refusal::Missing))
}

/// Answers a `Refusal::Missing` with [`SUBSTITUTE`], encoded by the link's codec; anything else
/// passes on.
struct Substitute;

impl ErrorHandler<Rpc> for Substitute {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        if !missing(&err) {
            return Err(err);
        }
        Ok(cx.reply(&SUBSTITUTE)?)
    }
}

/// Answers a `Refusal::Missing` with a stream of [`SUBSTITUTE_ITEMS`]; anything else passes on.
struct SubstituteStream;

impl ErrorHandler<Rpc> for SubstituteStream {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<Reply, BoxError> {
        if !missing(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::iter(SUBSTITUTE_ITEMS.map(Ok::<_, Refusal>))))
    }
}

/// Refuses every call.
pub(crate) struct Deny;

impl Guard<Rpc> for Deny {
    async fn can_activate(&self, _cx: &RpcCx) -> Result<bool, BoxError> {
        Ok(false)
    }
}

/// The handlers wait on the app's clock, the scenario's runtime, taken as a `Dep<dyn Timer>`
/// parameter.
#[injectable]
pub(crate) struct CoreController {
    probe: Dep<Probe>,
}

#[routes]
impl CoreController {
    #[ulo_rpc::message("conformance.add")]
    async fn add(&self, req: Payload<Add>, _end: Dep<AddEnd>) -> Result<Sum, Refusal> {
        self.probe.0.adds_begun.fetch_add(1, Ordering::SeqCst);
        Ok(Sum { sum: req.0.a + req.0.b })
    }

    #[ulo_rpc::message("conformance.echo")]
    async fn echo(&self, text: Payload<String>) -> Result<String, Refusal> {
        Ok(text.0)
    }

    #[ulo_rpc::message("conformance.conflict")]
    async fn conflict(&self) -> Result<(), Refusal> {
        Err(Refusal::Conflict)
    }

    #[ulo_rpc::message("conformance.substituted")]
    #[error_handlers(value = Substitute)]
    async fn substituted(&self) -> Result<Sum, Refusal> {
        Err(Refusal::Missing)
    }

    #[ulo_rpc::message("conformance.guarded")]
    #[guards(value = Deny)]
    async fn guarded(&self) -> Result<(), Refusal> {
        Ok(())
    }

    #[ulo_rpc::message("conformance.boom")]
    async fn boom(&self) -> Result<(), Refusal> {
        panic!("the conformance handler panics")
    }

    #[ulo_rpc::message("conformance.headers")]
    async fn headers(&self, headers: CallHeaders) -> Result<Option<String>, Refusal> {
        Ok(headers.get(HEADER).map(str::to_owned))
    }

    /// Answers with what only the call's own context knows: the pattern it named, the link it
    /// arrived on and the header it carried.
    #[ulo_rpc::message("conformance.context")]
    async fn context(&self, cx: RpcCx) -> Result<(String, String, Option<String>), Refusal> {
        Ok((cx.pattern().to_owned(), cx.link().name().to_owned(), cx.headers().get(HEADER).map(str::to_owned)))
    }

    /// Answers with a payload the link's codec cannot encode, the server's own failure.
    #[ulo_rpc::message("conformance.unencodable")]
    async fn unencodable(&self) -> Result<Data, Refusal> {
        Ok(Data::new(GARBLED))
    }

    #[ulo_rpc::event("conformance.event")]
    async fn event(&self, value: Payload<u32>) -> Result<(), Refusal> {
        *lock(&self.probe.0.last_event) = Some(value.0);
        self.probe.0.events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Runs for the given milliseconds, recording when it began, its deadline then, and whether it
    /// completed or its future was dropped first.
    #[ulo_rpc::event("conformance.slow_event")]
    async fn slow_event(&self, millis: Payload<u64>, exec: ExecutionRef, timer: Dep<dyn Timer>) -> Result<(), Refusal> {
        let left = exec.deadline().map(|deadline| deadline.saturating_duration_since(timer.now()));
        let run = {
            let mut runs = lock(&self.probe.0.slow);
            runs.push(SlowRun { began: Instant::now(), left, ended: None });
            runs.len() - 1
        };
        let mut guard = SlowGuard { exec, probe: (*self.probe).clone(), run, completed: false };
        timer.sleep(Duration::from_millis(millis.0)).await;
        guard.completed = true;
        Ok(())
    }

    #[ulo_rpc::event("conformance.tally")]
    async fn tally(&self, _value: Payload<u32>) -> Result<(), Refusal> {
        self.probe.0.tallies.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    /// Waits until its execution is cancelled, by `deadline-ms` or the caller's `cancel`.
    #[ulo_rpc::message("conformance.stall")]
    async fn stall(&self, exec: ExecutionRef, timer: Dep<dyn Timer>) -> Result<(), Refusal> {
        let _recorder = Recorder { exec: exec.clone(), probe: (*self.probe).clone(), stream: false };
        select(pin!(exec.cancelled()), timer.sleep(Duration::from_secs(30))).await;
        Ok(())
    }

    /// Answers nothing while its server serves: it returns when its execution is cancelled or the
    /// server drains, so the server's stop does not wait out the drain window for it.
    #[ulo_rpc::message("conformance.never")]
    async fn never(&self, exec: ExecutionRef) -> Result<(), Refusal> {
        self.probe.0.unanswered.fetch_add(1, Ordering::SeqCst);
        select(pin!(exec.cancelled()), pin!(exec.draining())).await;
        Ok(())
    }

    #[ulo_rpc::message("conformance.hold")]
    async fn hold(&self, millis: Payload<u64>, timer: Dep<dyn Timer>) -> Result<u64, Refusal> {
        self.probe.0.holding.fetch_add(1, Ordering::SeqCst);
        timer.sleep(Duration::from_millis(millis.0)).await;
        *lock(&self.probe.0.held_answered) = Some(Instant::now());
        Ok(millis.0)
    }

    /// Holds for the given milliseconds, counted with [`bounded_too`](Self::bounded_too) for the
    /// in-flight scenario: two patterns, so a window kept per pattern shows.
    #[ulo_rpc::message("conformance.bounded")]
    async fn bounded(&self, millis: Payload<u64>, timer: Dep<dyn Timer>) -> Result<u64, Refusal> {
        Ok(self.probe.bounded(&*timer, millis.0).await)
    }

    #[ulo_rpc::message("conformance.bounded_too")]
    async fn bounded_too(&self, millis: Payload<u64>, timer: Dep<dyn Timer>) -> Result<u64, Refusal> {
        Ok(self.probe.bounded(&*timer, millis.0).await)
    }

    /// Records the deadline its server computed for the call, and answers.
    #[ulo_rpc::message("conformance.deadline")]
    async fn deadline(&self, exec: ExecutionRef, headers: CallHeaders) -> Result<(), Refusal> {
        self.probe.see(&exec, &headers);
        Ok(())
    }
}

#[injectable]
pub(crate) struct StreamController {
    probe: Dep<Probe>,
}

#[routes]
impl StreamController {
    #[ulo_rpc::message("conformance.count")]
    async fn count(&self, n: Payload<u32>) -> impl Stream<Item = Result<u32, Refusal>> {
        stream::iter((1..=n.0).map(Ok))
    }

    #[ulo_rpc::message("conformance.sum")]
    async fn sum(&self, items: Inbound<i64>) -> Result<i64, Refusal> {
        let mut items = items;
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item.map_err(|_| Refusal::BadItem)?;
        }
        Ok(total)
    }

    /// The streamed request's items in the order they arrived, answered once `in_end` ends the
    /// stream: an item arriving after `in_end` is not among them.
    #[ulo_rpc::message("conformance.collect")]
    async fn collect(&self, items: Inbound<u32>) -> Result<Vec<u32>, Refusal> {
        let mut items = items;
        let mut arrived = Vec::new();
        while let Some(item) = items.next().await {
            arrived.push(item.map_err(|_| Refusal::BadItem)?);
        }
        Ok(arrived)
    }

    #[ulo_rpc::message("conformance.double")]
    async fn double(&self, items: Inbound<i64>) -> impl Stream<Item = Result<i64, Refusal>> {
        items.map(|item| item.map(|n| n * 2).map_err(|_| Refusal::BadItem))
    }

    #[ulo_rpc::message("conformance.substituted_stream")]
    #[error_handlers(value = SubstituteStream)]
    async fn substituted_stream(&self) -> Result<stream::Empty<Result<u32, Refusal>>, Refusal> {
        Err(Refusal::Missing)
    }

    /// One item, then a payload the link's codec cannot encode.
    #[ulo_rpc::message("conformance.unencodable_stream")]
    async fn unencodable_stream(&self, cx: RpcCx) -> Result<Reply, Refusal> {
        let first = cx.codec().encode(&1u32).expect("a `u32` encodes in every codec");
        let items: BoxStream<'static, Result<Data, BoxError>> = Box::pin(stream::iter([Ok(first), Ok(Data::new(GARBLED))]));
        Ok(Reply::Many(items))
    }

    /// Ticks until the stream is dropped, recording the reason when it is.
    #[ulo_rpc::message("conformance.ticks")]
    async fn ticks(&self, exec: ExecutionRef, timer: Dep<dyn Timer>) -> impl Stream<Item = Result<u64, Refusal>> {
        let recorder = Recorder { exec, probe: (*self.probe).clone(), stream: true };
        stream::unfold((0u64, recorder, timer), |(n, recorder, timer)| async move {
            timer.sleep(Duration::from_millis(20)).await;
            Some((Ok(n), (n + 1, recorder, timer)))
        })
    }

    /// Yields `0`, falls silent for the given milliseconds, yields `1`, and ends.
    #[ulo_rpc::message("conformance.pause")]
    async fn pause(&self, millis: Payload<u64>, timer: Dep<dyn Timer>) -> impl Stream<Item = Result<u64, Refusal>> {
        let silence = Duration::from_millis(millis.0);
        stream::unfold((0u64, timer), move |(n, timer)| async move {
            match n {
                0 => Some((Ok(0), (1, timer))),
                1 => {
                    timer.sleep(silence).await;
                    Some((Ok(1), (2, timer)))
                }
                _ => None,
            }
        })
    }

    /// Records the deadline its server computed for the stream, and yields `1`.
    #[ulo_rpc::message("conformance.deadline_stream")]
    async fn deadline_stream(&self, exec: ExecutionRef, headers: CallHeaders) -> impl Stream<Item = Result<u32, Refusal>> {
        self.probe.see(&exec, &headers);
        stream::iter([Ok(1)])
    }

    /// Records the deadline its server computed for the streamed request, and answers its items'
    /// sum.
    #[ulo_rpc::message("conformance.deadline_sum")]
    async fn deadline_sum(&self, items: Inbound<i64>, exec: ExecutionRef, headers: CallHeaders) -> Result<i64, Refusal> {
        self.probe.see(&exec, &headers);
        let mut items = items;
        let mut total = 0;
        while let Some(item) = items.next().await {
            total += item.map_err(|_| Refusal::BadItem)?;
        }
        Ok(total)
    }

    /// Ticks until the server drains, then ends cleanly.
    #[ulo_rpc::message("conformance.until_drain")]
    async fn until_drain(&self, exec: ExecutionRef, timer: Dep<dyn Timer>) -> impl Stream<Item = Result<u64, Refusal>> {
        stream::unfold((0u64, exec, timer), |(n, exec, timer)| async move {
            if exec.is_draining() {
                return None;
            }
            timer.sleep(Duration::from_millis(20)).await;
            if exec.is_draining() { None } else { Some((Ok(n), (n + 1, exec, timer))) }
        })
    }
}

#[injectable]
pub(crate) struct BinaryController;

#[routes]
impl BinaryController {
    #[ulo_rpc::message("conformance.bytes")]
    async fn bytes(&self, data: Payload<Bytes>) -> Result<Bytes, Refusal> {
        Ok(data.0)
    }
}

/// Which controllers a server mounts.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Mounts {
    pub(crate) streaming: bool,
    pub(crate) binary: bool,
}

impl Mounts {
    /// What the link carries: every scenario it can run.
    pub(crate) fn carried(capabilities: &Capabilities) -> Mounts {
        Mounts { streaming: streams(capabilities), binary: capabilities.binary }
    }
}

/// Whether the link carries every streamed shape.
pub(crate) fn streams(capabilities: &Capabilities) -> bool {
    [Shape::ServerStreaming, Shape::ClientStreaming, Shape::Bidi].iter().all(|shape| capabilities.shapes.contains(shape))
}

struct ServerRoot {
    probe: Probe,
    mounts: Mounts,
}

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.probe.clone());
        m.provide::<AddEnd>();
        m.controller::<CoreController>();
        if self.mounts.streaming {
            m.controller::<StreamController>();
        }
        if self.mounts.binary {
            m.controller::<BinaryController>();
        }
    }
}

/// One running server instance and what its handlers record.
pub(crate) struct Server {
    pub(crate) handle: AppHandle,
    pub(crate) probe: Probe,
    /// What the app reported bound once it listened, for `Broker::client_link`.
    pub(crate) addresses: Vec<BoundAddr>,
    serving: ValueHandle<()>,
    /// How `serve` returned, once it has: a server whose link failed after `listen` stops serving
    /// here, and [`ready`] reports it.
    ended: Arc<Mutex<Option<String>>>,
}

impl Server {
    pub(crate) async fn stop(self) {
        let _ = self.handle.close(Signal::new("conformance")).await;
        let _ = self.serving.await;
    }
}

async fn bound<B: Broker, L: Link>(
    link: L,
    probe: Probe,
    mounts: Mounts,
    max_inflight: Count,
    cap: Option<Duration>,
) -> Result<App<Serving>, StartupError> {
    let mut builder = App::builder(ServerRoot { probe, mounts }).runtime(<B::Harness as Harness>::runtime()).drain_timeout(DRAIN);
    if let Some(cap) = cap {
        builder = builder.shutdown_timeout(cap);
    }
    builder
        .wire()?
        .connect()
        .await?
        .bind(ulo_rpc::Server::new(link).max_inflight(max_inflight))
        .listen()
        .await
}

/// A server mounting what the link carries, serving until stopped.
pub(crate) async fn server<B: Broker>(broker: &B) -> Server {
    server_bounded(broker, Count::Default).await
}

/// [`server`] with its `max_inflight` set.
pub(crate) async fn server_bounded<B: Broker>(broker: &B, max_inflight: Count) -> Server {
    server_over(broker, broker.link(), max_inflight).await
}

/// [`server_bounded`] over `link`, a link built from the broker's [`Broker::link`].
pub(crate) async fn server_over<B: Broker, L: Link>(broker: &B, link: L, max_inflight: Count) -> Server {
    serving::<B, L>(broker, link, max_inflight, None).await
}

/// [`server`] with its `shutdown_timeout` set to `cap`.
pub(crate) async fn server_capped<B: Broker>(broker: &B, cap: Duration) -> Server {
    serving::<B, _>(broker, broker.link(), Count::Default, Some(cap)).await
}

async fn serving<B: Broker, L: Link>(broker: &B, link: L, max_inflight: Count, cap: Option<Duration>) -> Server {
    let probe = Probe::default();
    let mounts = Mounts::carried(&broker.link().capabilities());
    let app = bound::<B, L>(link, probe.clone(), mounts, max_inflight, cap)
        .await
        .unwrap_or_else(|error| crate::startup_failed!("the conformance server did not start: {}", report(&error)));
    let handle = app.handle();
    let addresses = app.addresses();
    let ended = Arc::new(Mutex::new(None));
    let serving = spawn::<B, _, _>({
        let ended = Arc::clone(&ended);
        async move {
            let outcome = match app.serve(std::future::pending::<Signal>()).await {
                Ok(shutdown) => format!("it shut down on `{}`", shutdown.signal),
                Err(error) => format!("its shutdown failed: {}", report(&error)),
            };
            *lock(&ended) = Some(outcome);
        }
    });
    Server { handle, probe, addresses, serving, ended }
}

/// Asserts that a server mounting `mounts` is refused at startup as a `Configure` error: what a
/// link's capabilities exclude is refused in `prepare`, before anything binds.
pub(crate) async fn refused_at_startup<B: Broker>(broker: &B, mounts: Mounts) {
    match bound::<B, _>(broker.link(), Probe::default(), mounts, Count::Default, None).await {
        Err(StartupError::Configure(_)) => {}
        Err(other) => crate::startup_failed!("expected a `Configure` refusal for {mounts:?}, got: {}", report(&other)),
        Ok(app) => {
            let _ = app.handle().close(Signal::new("conformance")).await;
            panic!("a server mounting {mounts:?} started on a link whose capabilities exclude it");
        }
    }
}

/// Imports `RpcClientModule` over the broker's client-side link.
struct ClientRoot<L> {
    link: Mutex<Option<L>>,
}

impl<L: Link> Module for ClientRoot<L> {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = lock(&self.link).take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(CALL_TIMEOUT)));
        }
    }
}

/// The caller's side: an app holding the client, kept alive beside it until `close`.
pub(crate) struct Client {
    pub(crate) rpc: RpcClient,
    app: App<Connected>,
    root: ModuleRef,
}

impl Client {
    /// Closes the client's app, whose `RpcClientModule` closes the link while the broker is still
    /// there to answer it.
    pub(crate) async fn close(self) {
        let _ = self.closed().await;
    }

    /// Runs `f` in a standalone execution of the client's app opened with `opts`, as code calling
    /// out from inside an execution does.
    pub(crate) async fn execute<R>(&self, opts: ExecOptions, f: impl AsyncFnOnce(&Execution) -> R) -> R {
        self.app.execute(opts, f).await.unwrap_or_else(|closed| panic!("the client's execution was refused: {closed}"))
    }

    /// An execution of the client's app opened with `opts`, as a transport opens one: nothing
    /// fires its cancellation when its deadline passes.
    pub(crate) fn open(&self, opts: ExecOptions) -> Execution {
        Execution::open(&self.root, opts).unwrap_or_else(|closed| panic!("the client's execution was refused: {closed}"))
    }

    /// Closes the client's app as [`close`](Self::close) does, answering its report: a destroy hook
    /// that timed out is one of its failures.
    pub(crate) async fn closed(self) -> Result<Shutdown, ShutdownError> {
        self.app.close(Signal::new("conformance")).await
    }
}

/// A client over the broker's client link, built once `servers` are bound so it can reach the
/// addresses they report.
pub(crate) async fn client<B: Broker>(broker: &B, servers: &[&Server]) -> Client {
    client_over::<B, _>(broker.client_link(&addresses(servers))).await
}

/// Every address `servers` bound, in the order they started.
pub(crate) fn addresses(servers: &[&Server]) -> Vec<BoundAddr> {
    servers.iter().flat_map(|server| server.addresses.iter().cloned()).collect()
}

/// A client over `link`, on the harness's runtime.
pub(crate) async fn client_over<B: Broker, L: Link>(link: L) -> Client {
    let app = App::builder(ClientRoot { link: Mutex::new(Some(link)) })
        .runtime(<B::Harness as Harness>::runtime())
        .wire()
        .unwrap_or_else(|error| crate::startup_failed!("the conformance client did not wire: {}", report(&error)))
        .connect()
        .await
        .unwrap_or_else(|error| crate::startup_failed!("the conformance client did not connect: {}", report(&error)));
    let rpc =
        app.get::<RpcClient>().await.unwrap_or_else(|error| crate::startup_failed!("the conformance client has no `RpcClient`: {}", report(&error)));
    let root = app.module::<ClientRoot<L>>().unwrap_or_else(|error| crate::startup_failed!("the conformance client has no root module: {}", report(&error)));
    Client { rpc: (*rpc).clone(), app, root }
}

/// One scenario's environment: a broker, one server and one client, the server answering.
pub(crate) struct Fixture<B: Broker> {
    pub(crate) broker: B,
    pub(crate) server: Server,
    pub(crate) client: Client,
}

impl<B: Broker> Fixture<B> {
    pub(crate) async fn start() -> Fixture<B> {
        Fixture::start_bounded(Count::Default).await
    }

    /// [`start`](Self::start) with the server's `max_inflight` set.
    pub(crate) async fn start_bounded(max_inflight: Count) -> Fixture<B> {
        Fixture::start_over(max_inflight, B::link).await
    }

    /// [`start`](Self::start) with the server's `shutdown_timeout` set to `cap`, returned once the
    /// readiness calls' executions have ended: a shutdown counts an execution its holder has not
    /// yet dropped as abandoned, and the answer reaches the caller before that.
    pub(crate) async fn start_capped(cap: Duration) -> Fixture<B> {
        let broker = B::start().await;
        let server = server_capped(&broker, cap).await;
        let client = client(&broker, &[&server]).await;
        ready(&broker, &client.rpc, &[&server]).await;
        within::<B, _>(Duration::from_secs(10), "the readiness calls' executions to end", async {
            while !server.probe.adds_settled() {
                sleep::<B>(Duration::from_millis(5)).await;
            }
        })
        .await;
        Fixture { broker, server, client }
    }

    /// [`start_bounded`](Self::start_bounded) with the server over the link `server_link` builds
    /// from the broker, the broker's own link inside something that watches it.
    pub(crate) async fn start_over<L: Link>(max_inflight: Count, server_link: impl FnOnce(&B) -> L) -> Fixture<B> {
        let broker = B::start().await;
        let link = server_link(&broker);
        let server = server_over(&broker, link, max_inflight).await;
        let client = client(&broker, &[&server]).await;
        ready(&broker, &client.rpc, &[&server]).await;
        Fixture { broker, server, client }
    }

    pub(crate) fn capabilities(&self) -> Capabilities {
        self.broker.link().capabilities()
    }

    pub(crate) fn rpc(&self) -> &RpcClient {
        &self.client.rpc
    }

    pub(crate) async fn stop(self) {
        self.server.stop().await;
        self.client.close().await;
    }
}

/// Waits until a unary call succeeds, within the broker's boot budget: a broker subscribes after
/// `bind` returns on some links, and a consumer group takes its partitions later still. On
/// failure it reports the last call's outcome and how each of `servers` stopped serving, if one
/// did.
pub(crate) async fn ready<B: Broker>(broker: &B, rpc: &RpcClient, servers: &[&Server]) {
    let deadline = Instant::now() + broker.budget().boot;
    loop {
        let outcome = rpc.request::<_, Sum>(ADD, &Add { a: 1, b: 2 }).timeout(Duration::from_millis(500)).await;
        match outcome {
            Ok(Sum { sum: 3 }) => return,
            outcome if Instant::now() >= deadline => {
                let stopped: Vec<String> = servers.iter().filter_map(|server| lock(&server.ended).clone()).collect();
                let stopped = if stopped.is_empty() {
                    "every server still serving".to_owned()
                } else {
                    format!("a server stopped serving: {}", stopped.join("; "))
                };
                crate::startup_failed!("the server did not answer within the boot budget: {outcome:?}, {stopped}")
            }
            _ => sleep::<B>(Duration::from_millis(100)).await,
        }
    }
}

/// Polls `done` until it holds or `within` passes; answers whether it held.
pub(crate) async fn eventually<B: Broker>(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep::<B>(Duration::from_millis(20)).await;
    }
}

/// The wait for an answer that may be the caller's own `Timeout`.
pub(crate) fn miss_wait<B: Broker>(broker: &B) -> Duration {
    broker.budget().settle.max(Duration::from_secs(1))
}

/// Asserts `outcome` failed with `kind`, and returns the error for further checks.
pub(crate) fn failed<T: fmt::Debug>(outcome: Result<T, RpcError>, kind: ErrorKind) -> RpcError {
    match outcome {
        Err(error) if error.kind() == kind => error,
        other => panic!("expected an error of kind {kind:?}, got: {other:?}"),
    }
}

/// Runs `call` to completion within `within`, failing the scenario when it does not.
pub(crate) async fn within<B: Broker, T>(within: Duration, what: &str, call: impl Future<Output = T>) -> T {
    timeout::<B, T>(within, call).await.unwrap_or_else(|| panic!("{what} did not finish within {within:?}"))
}

/// A spawned task's value, or a failure naming how the task ended instead.
pub(crate) fn joined<T>(ended: Result<T, TaskEnd>, what: &str) -> T {
    ended.unwrap_or_else(|end| panic!("{what} did not return a value: it ended {end:?}"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
