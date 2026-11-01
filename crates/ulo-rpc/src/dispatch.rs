//! Per-delivery execution (transports DESIGN §5.2): one execution per call, `deadline-ms` as its
//! deadline, a `deadline-ms` the server cannot read refused before any execution opens,
//! `cancel` as `ClientCancelled`, the four shapes, `dispatch_late` for an item's error,
//! and the split per `Capabilities` between the whole frame and the payload with native
//! correlation. `Server::serve` runs it for every delivery the link's inbound stream yields.
//!
//! A deadline that passes before the call answers drops the pipeline at its await, cancels the
//! execution with `CancelReason::Deadline`, and offers the error handlers, the handler's tiers then
//! the global ones, a `Timeout` under `Server::timeout_grace`: what they answer, or the error they
//! return written as it stands, is the reply; unclaimed, or with no answer by the end of the
//! grace, the call answers `err` of kind `timeout`. A stream they answer is dropped unread, its
//! `on_stream_end` reporting `CutOff(Deadline)`, and the call answers that `err` too, logged at
//! `warn`: the caller stopped waiting at the deadline, and the grace bounds the error handlers, not
//! a stream they return. A deadline passing once a
//! streamed reply began ends it with that `err`, offered to no error handler.
//!
//! The serve loop routes every frame in arrival order before anything awaits: a `req`, `evt` or
//! `open` opens its execution and registers its id there, so an `in`, `in_end` or `cancel` the
//! link delivers right after it always finds the call; the call itself then runs in its own task.

use std::collections::HashMap;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::task::Poll;
use std::time::Duration;

use futures_core::stream::BoxStream;
use futures_util::StreamExt;
use futures_util::future::{Either, select};
use tracing::{Instrument, Span};
use ulo::{
    AppHandle, BoxError, BoxFuture, CancelReason, EndStream, ExecOptions, Execution, ExecutionRef, LateOutcome,
    MountedHandler, Runtime, Shape, Timer, TraceMode, Transport,
};
use ulo_transport::{Admission, CallError, Detail, Details, ErrorKind, Permit, TaskSet, Tracked, Turns, span};
use ulo_transport::__private::Watch;

use crate::handler::{HandlerFn, Kind};
use crate::codec::Codec;
use crate::frame::{Data, ErrorBody, Frame};
use crate::link::{Ack, Capabilities, Delivery, DeliveryMode, FrameTooLarge, FrameUnencodable, Pattern, ReplyPath};
use crate::queue::{self, Feed, Halted};
use crate::trace;
use crate::transport::{Body, CallHeaders, CxInner, LinkInfo, NoHandler, Reply, Rpc, RpcCx};

/// The `ErrorInfo` domain of every reason this transport writes.
pub(crate) const DOMAIN: &str = "ulo.rpc";

/// What `Server::prepare` built, read by every delivery.
pub(crate) struct Shared {
    pub(crate) app: AppHandle,
    pub(crate) timer: Arc<dyn Timer>,
    /// What every call's task is spawned on.
    pub(crate) runtime: Arc<dyn Runtime>,
    pub(crate) routes: HashMap<String, Route>,
    pub(crate) codec: Codec,
    pub(crate) capabilities: Capabilities,
    /// `Link::NAME`.
    pub(crate) link: &'static str,
    pub(crate) admission: Admission,
    /// `Server::timeout_grace` as a duration; `None` waits for the error handlers.
    pub(crate) grace: Option<Duration>,
    /// `Server::max_queued_items` as a limit; `None` for `Count::Unlimited`.
    pub(crate) queued: Option<usize>,
    pub(crate) calls: Mutex<HashMap<u64, Live>>,
    pub(crate) serial: AtomicU64,
    /// Events no handler took, counted for the log line each one writes.
    pub(crate) unhandled_events: AtomicU64,
    /// What `Server::drain` waits for beside the live executions the core waits for.
    pub(crate) settling: Watch<Settling>,
    /// Every call's task, which `Server::close` polls once before the link closes.
    pub(crate) turns: Turns,
}

/// What the drain window waits for that opens no execution: the link's inbound stream read to
/// its end, since a delivery it holds is a call nobody has answered, and the refusals written
/// for calls that could not open one.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Settling {
    /// `serve` holds the inbound stream and reads it; a server that never served has nothing to
    /// wait for.
    pub(crate) reading: bool,
    /// The inbound stream returned `None`.
    pub(crate) ended: bool,
    /// Refusals spawned and not yet sent.
    pub(crate) refusals: usize,
}

impl Settling {
    /// Nothing that reached the server is left unanswered.
    pub(crate) fn settled(&self) -> bool {
        !self.reading || (self.ended && self.refusals == 0)
    }
}

/// One pattern's handler.
#[derive(Clone)]
pub(crate) struct Route {
    pub(crate) handler: MountedHandler<Rpc>,
    pub(crate) call: HandlerFn,
    pub(crate) kind: Kind,
    pub(crate) shape: Shape,
}

/// A call in flight, by its id: what a `cancel` cancels and where an `in` goes.
pub(crate) struct Live {
    /// Tells this registration from a later one under the same id.
    serial: u64,
    exec: ExecutionRef,
    /// `None` once `in_end` arrived or the queue overflowed, and for a call whose request is not
    /// streamed.
    inbound: Option<Feed<Data>>,
}

impl Shared {
    fn calls(&self) -> MutexGuard<'_, HashMap<u64, Live>> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Which frame opened a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Arrived {
    Req,
    Evt,
    Open,
}

struct Incoming {
    id: Option<u64>,
    arrived: Arrived,
    pattern: String,
    headers: CallHeaders,
    body: Body,
    feed: Option<Feed<Data>>,
}

/// Routes one delivery, in arrival order: a call is opened and spawned into `tasks`; a frame for a
/// call in flight reaches it at once.
pub(crate) fn accept(shared: &Arc<Shared>, delivery: Delivery, tasks: &mut TaskSet) {
    let Delivery { frame, reply, ack } = delivery;
    match frame {
        Frame::Req { id, pattern, headers, data } => {
            let incoming = Incoming { id: Some(id), arrived: Arrived::Req, pattern, headers, body: Body::Data(data), feed: None };
            start(shared, incoming, reply, ack, tasks);
        }
        Frame::Evt { pattern, headers, data } => {
            let incoming = Incoming { id: None, arrived: Arrived::Evt, pattern, headers, body: Body::Data(data), feed: None };
            start(shared, incoming, reply, ack, tasks);
        }
        Frame::Open { id, pattern, headers } => {
            let (feed, items) = queue::queue(shared.queued);
            let incoming =
                Incoming { id: Some(id), arrived: Arrived::Open, pattern, headers, body: Body::Stream(items), feed: Some(feed) };
            start(shared, incoming, reply, ack, tasks);
        }
        Frame::In { id, data } => {
            // Offered, never awaited: waiting for room here would stall every call on the link
            // behind one handler that does not read.
            let overflowed = {
                let mut calls = shared.calls();
                match calls.get_mut(&id) {
                    Some(live) if live.inbound.as_ref().is_some_and(|feed| !feed.offer(data)) => {
                        if let Some(feed) = live.inbound.take() {
                            feed.halt();
                        }
                        Some(live.exec.clone())
                    }
                    _ => None,
                }
            };
            // The halt wakes a reader on any task, and its next read fails; the call's task answers
            // `overflow` whether the cancellation or the handler's answer reaches it first, and
            // the client's pump stops sending when that answer ends the call.
            if let Some(exec) = overflowed {
                exec.cancel_with(CancelReason::Explicit);
            }
            ack.ack();
        }
        Frame::InEnd { id } => {
            if let Some(live) = shared.calls().get_mut(&id) {
                live.inbound = None;
            }
            ack.ack();
        }
        Frame::Cancel { id } => {
            let reason = if reply.is_some() { CancelReason::ClientCancelled } else { CancelReason::Disconnected };
            let exec = shared.calls().get(&id).map(|live| live.exec.clone());
            if let Some(exec) = exec {
                exec.cancel_with(reason);
            }
            ack.ack();
        }
        other => {
            tracing::debug!(kind = other.kind(), link = shared.link, "a frame the server does not take arrived and was dropped");
            ack.ack();
        }
    }
}

fn start(shared: &Arc<Shared>, incoming: Incoming, reply: Option<ReplyPath>, ack: Ack, tasks: &mut TaskSet) {
    let link = LinkInfo { name: shared.link, peer: reply.as_ref().and_then(ReplyPath::peer_addr) };
    // An event's reply path carries its peer and nothing else.
    let reply = if incoming.arrived == Arrived::Evt { None } else { reply };
    let Some(route) = shared.routes.get(incoming.pattern.as_str()) else {
        return unhandled(shared, incoming, link, reply, ack, tasks);
    };
    let answered = incoming.id.zip(reply);
    // Before admission: the refusal is permanent, and a caller told `unavailable` first would
    // retry a call the server will never run.
    let deadline = match incoming.headers.deadline() {
        Ok(deadline) => deadline,
        Err(value) => {
            let error = bad_deadline(value);
            match answered {
                Some((id, reply)) => {
                    refuse(shared, tasks, reply, id, error);
                    ack.ack();
                }
                None => {
                    tracing::warn!(pattern = incoming.pattern.as_str(), link = shared.link, message = %error.message, "an event whose `deadline-ms` the server cannot read was rejected");
                    ack.reject();
                }
            }
            return;
        }
    };
    let Some(permit) = shared.admission.try_admit() else {
        match answered {
            Some((id, reply)) => {
                let error = refusal("the server is over its in-flight limit").with(Detail::RetryAfter(shared.admission.shed_retry_after()));
                refuse(shared, tasks, reply, id, error);
            }
            // Left unsettled, so a broker redelivers it rather than dropping it.
            None => tracing::warn!(pattern = incoming.pattern.as_str(), link = shared.link, "an event over the in-flight limit was left unacknowledged"),
        }
        return;
    };
    let opts = match deadline {
        Some(after) => ExecOptions::new().deadline(shared.timer.now() + after),
        None => ExecOptions::new(),
    };
    // An event takes the same path, so its handler's span is a child of the emitter's.
    let opts = traced(shared, &incoming.headers, opts);
    let Ok(exec) = Execution::open(route.handler.module(), opts) else {
        if let Some((id, reply)) = answered {
            refuse(shared, tasks, reply, id, refusal(ulo_transport::SHUTDOWN_MESSAGE));
        }
        return;
    };
    exec.seed(incoming.headers.clone());
    exec.seed(link.clone());
    let handle = exec.handle();
    let halted = incoming.feed.as_ref().map(Feed::halted);
    let registered = incoming.id.map(|id| {
        let serial = shared.serial.fetch_add(1, Ordering::Relaxed);
        shared.calls().insert(id, Live { serial, exec: handle.clone(), inbound: incoming.feed });
        Registered { shared: Arc::clone(shared), id, serial }
    });
    let cx = context(shared, handle, &incoming.pattern, incoming.headers, link, incoming.body);
    let call_span = call_span(shared, &exec, incoming.arrived, &incoming.pattern, Some(&route.handler));
    let call = Call {
        shared: Arc::clone(shared),
        route: route.clone(),
        exec,
        cx,
        arrived: incoming.arrived,
        answered,
        ack,
        deadline,
        halted,
        _registered: registered,
        permit,
    };
    tasks.spawn(shared.turns.track(call.run().instrument(call_span)));
}

fn context(shared: &Shared, exec: ExecutionRef, pattern: &str, headers: CallHeaders, link: LinkInfo, body: Body) -> RpcCx {
    RpcCx::new(CxInner {
        exec,
        pattern: Pattern::from(pattern),
        headers,
        link,
        app: shared.app.clone(),
        timer: Arc::clone(&shared.timer),
        codec: shared.codec,
        body: Mutex::new(Some(body)),
    })
}

/// A request or streamed request naming a pattern nothing handles: the global error handlers
/// receive `Unavailable` whose source is [`NoHandler`], answered `reason: "pattern_unhandled"`
/// when none claims it. An event is logged, counted and rejected, so a broker cannot loop it.
fn unhandled(shared: &Arc<Shared>, incoming: Incoming, link: LinkInfo, reply: Option<ReplyPath>, ack: Ack, tasks: &mut TaskSet) {
    let Some((id, reply)) = incoming.id.zip(reply).filter(|_| incoming.arrived != Arrived::Evt) else {
        let count = shared.unhandled_events.fetch_add(1, Ordering::Relaxed) + 1;
        tracing::warn!(pattern = incoming.pattern.as_str(), link = shared.link, unhandled = count, "an event no handler takes was rejected");
        ack.reject();
        return;
    };
    let opts = traced(shared, &incoming.headers, ExecOptions::new());
    let Ok(exec) = Execution::open(&shared.app.root(), opts) else {
        refuse(shared, tasks, reply, id, refusal(ulo_transport::SHUTDOWN_MESSAGE));
        return;
    };
    exec.seed(incoming.headers.clone());
    exec.seed(link.clone());
    let error = CallError::new(ErrorKind::Unavailable, format!("no handler for pattern `{}`", incoming.pattern))
        .with_details(Details::new().with(reason("pattern_unhandled")))
        .with_source(NoHandler::new(&incoming.pattern));
    let cx = context(shared, exec.handle(), &incoming.pattern, incoming.headers, link, incoming.body);
    let shared = Arc::clone(shared);
    let call_span = call_span(&shared, &exec, incoming.arrived, cx.pattern(), None);
    let turns = shared.turns.clone();
    tasks.spawn(
        turns.track(
            async move {
                let handle = exec.handle();
                let outcome = ulo::recover::<Rpc>(None, &handle, &cx, BoxError::from(error)).await;
                answer(&shared, None, &handle, &cx, &reply, id, Outcome::Done(outcome), &mut None, None).await;
                ack.ack();
                drop(exec);
            }
            .instrument(call_span),
        ),
    );
}

/// One call, run in its own task.
struct Call {
    shared: Arc<Shared>,
    route: Route,
    /// Held until the answer is written, a streamed reply's last item included: dropping it ends
    /// the execution, whatever handles remain.
    exec: Execution,
    cx: RpcCx,
    arrived: Arrived,
    /// The id and reply path of a call that answers; `None` for an event.
    answered: Option<(u64, ReplyPath)>,
    ack: Ack,
    deadline: Option<Duration>,
    /// Whether the streamed request's queue overflowed, for a call whose request is streamed.
    halted: Option<Halted>,
    _registered: Option<Registered>,
    permit: Permit,
}

impl Call {
    async fn run(self) {
        let Call { shared, route, exec, cx, arrived, answered, ack, deadline, halted, _registered, permit } = self;
        let handle = exec.handle();
        let mut deadline = deadline.map(|after| shared.timer.sleep(after));
        let outcome = match mismatch(arrived, &route, cx.pattern()) {
            Some(text) => {
                let error = BoxError::from(CallError::new(ErrorKind::BadRequest, text));
                Outcome::Done(ulo::recover(Some(&route.handler), &handle, &cx, error).await)
            }
            None => {
                let call = Arc::clone(&route.call);
                let dispatching = ulo::dispatch(&route.handler, &handle, &cx, move |cx| (*call)(cx));
                match until_ended(&handle, &mut deadline, dispatching).await {
                    Ended::Done(_) if overflowed(halted.as_ref()) => Outcome::Cancelled(cut_short(&handle)),
                    Ended::Done(outcome) => Outcome::Done(outcome),
                    Ended::Cancelled(Some(CancelReason::Deadline)) => {
                        Outcome::Expired(expired(&shared, &route.handler, &handle, &cx).await)
                    }
                    Ended::Cancelled(reason) => Outcome::Cancelled(reason),
                }
            }
        };
        match answered {
            Some((id, reply)) => {
                // A request to an event handler is answered with an empty `res` once it ran.
                let outcome = match outcome {
                    Outcome::Done(Ok(_)) if route.kind == Kind::Event => Outcome::Done(Ok(Reply::None)),
                    Outcome::Expired(Some(Ok(_))) if route.kind == Kind::Event => Outcome::Expired(Some(Ok(Reply::None))),
                    outcome => outcome,
                };
                let refusal = match &outcome {
                    Outcome::Done(Ok(reply)) => shape_mismatch(route.shape, reply),
                    _ => None,
                };
                match refusal {
                    Some(answered) => {
                        let handler = route.handler.name();
                        tracing::warn!(
                            pattern = cx.pattern(),
                            handler,
                            declared = ?route.shape,
                            answered,
                            "a handler answered in a shape other than the one it declares; the call was answered `internal`"
                        );
                        drop(outcome);
                        let message = format!("handler `{handler}` is declared {:?} and answered with {answered}", route.shape);
                        let error = ErrorBody::new(ErrorKind::Internal, message, Details::new().with(reason("shape_mismatch")));
                        send(&reply, &handle, id, Frame::Err { id, error }).await;
                    }
                    None => answer(&shared, Some(&route.handler), &handle, &cx, &reply, id, outcome, &mut deadline, halted.as_ref()).await,
                }
                // Freed before the settlement: a link declaring `native_backpressure` lets its
                // broker hand over the next request once the `Ack` settles, and that request must
                // find the place free.
                drop(permit);
                ack.ack();
            }
            None => {
                drop(permit);
                settle_event(&shared, cx.pattern(), &handle, outcome, ack);
            }
        }
        drop(exec);
    }
}

/// How a call's pipeline ended, as `answer` and `settle_event` write it.
enum Outcome {
    /// The pipeline's own result, an error rendered by [`render`].
    Done(Result<Reply, BoxError>),
    /// Cancelled before it answered, for any reason but the deadline.
    Cancelled(Option<CancelReason>),
    /// The deadline passed before it answered: what the error handlers answered to the `Timeout`
    /// offered them, an error written as it stands; `None` when the grace ran out first.
    Expired(Option<Result<Reply, BoxError>>),
}

/// The error handlers of `handler` over the `Timeout` a passed deadline offers them, under the
/// server's grace; `None` when the grace runs out first, the recovery dropped at its await.
async fn expired(shared: &Shared, handler: &MountedHandler<Rpc>, exec: &ExecutionRef, cx: &RpcCx) -> Option<Result<Reply, BoxError>> {
    let recovering = ulo::recover(Some(handler), exec, cx, BoxError::from(timeout_error()));
    match shared.grace {
        // The recovery is polled before the grace's sleep.
        Some(grace) => match select(pin!(recovering), shared.timer.sleep(grace)).await {
            Either::Left((outcome, _)) => Some(outcome),
            Either::Right(_) => None,
        },
        None => Some(recovering.await),
    }
}

/// A frame that does not fit the handler's shape: `req` or `evt` to a streamed-request pattern,
/// `open` to one that takes one payload or to an event handler.
fn mismatch(arrived: Arrived, route: &Route, pattern: &str) -> Option<String> {
    let streamed = matches!(route.shape, Shape::ClientStreaming | Shape::Bidi);
    match arrived {
        Arrived::Req | Arrived::Evt if streamed => Some(format!("pattern `{pattern}` takes a streamed request; open it with `open`")),
        Arrived::Open if !streamed || route.kind == Kind::Event => {
            Some(format!("pattern `{pattern}` takes one payload; send it with `req` or `evt`"))
        }
        _ => None,
    }
}

/// Whether `reply` is a stream from a handler whose declared shape answers one payload. The
/// reverse is not refused: an interceptor or an error handler may answer a streamed call with one
/// payload in place of the stream.
fn shape_mismatch(declared: Shape, reply: &Reply) -> Option<&'static str> {
    let streams = matches!(declared, Shape::ServerStreaming | Shape::Bidi);
    match reply {
        Reply::Many(_) if !streams => Some("a stream"),
        _ => None,
    }
}

/// An event's acknowledgment once its handler completed: acknowledged when it succeeded, or when
/// an error handler answered its passed deadline; rejected without requeue when it failed or its
/// deadline passed otherwise; and left unsettled when the call was cancelled for another reason,
/// so a broker redelivers it.
fn settle_event(shared: &Shared, pattern: &str, exec: &ExecutionRef, outcome: Outcome, ack: Ack) {
    // Called from the call's task, which runs instrumented with the call's span, so the current
    // span is the event's consumer span.
    let call_span = Span::current();
    match outcome {
        Outcome::Done(Ok(_)) | Outcome::Expired(Some(Ok(_))) => ack.ack(),
        Outcome::Done(Err(err)) => {
            let error = render(err, exec);
            trace::record_failure(&call_span, error.kind);
            tracing::warn!(pattern, link = shared.link, kind = %error.kind, message = %error.message, "an event's handler failed; the event was rejected");
            ack.reject();
        }
        Outcome::Expired(Some(Err(err))) => {
            let error = body_of(&CallError::from_boxed(err));
            trace::record_failure(&call_span, error.kind);
            tracing::warn!(pattern, link = shared.link, kind = %error.kind, message = %error.message, "an event's deadline passed; the event was rejected");
            ack.reject();
        }
        Outcome::Expired(None) => {
            trace::record_failure(&call_span, ErrorKind::Timeout);
            ack.reject();
        }
        Outcome::Cancelled(_) => {}
    }
}

/// Writes a call's outcome on its reply path.
#[allow(clippy::too_many_arguments)]
async fn answer(
    shared: &Shared,
    handler: Option<&MountedHandler<Rpc>>,
    exec: &ExecutionRef,
    cx: &RpcCx,
    reply: &ReplyPath,
    id: u64,
    outcome: Outcome,
    deadline: &mut Option<BoxFuture<'static, ()>>,
    halted: Option<&Halted>,
) {
    match outcome {
        Outcome::Done(Ok(Reply::None)) | Outcome::Expired(Some(Ok(Reply::None))) => {
            send(reply, exec, id, Frame::Res { id, data: Data::default() }).await;
        }
        Outcome::Done(Ok(Reply::One(data))) | Outcome::Expired(Some(Ok(Reply::One(data)))) => {
            send(reply, exec, id, Frame::Res { id, data }).await;
        }
        Outcome::Done(Ok(Reply::Many(stream))) => {
            stream_reply(shared, handler, exec, cx, reply, id, stream, deadline, halted).await;
        }
        Outcome::Expired(Some(Ok(Reply::Many(stream)))) => {
            // The error handlers answered with this stream, so it is the reply, dropped unwritten:
            // tracked for that drop, it reports `CutOff(Deadline)`.
            drop(Tracked::new(stream, exec.clone()));
            tracing::warn!(
                pattern = cx.pattern(),
                handler = handler.map(MountedHandler::name),
                "an error handler answered a timed-out call with a stream; the stream was ended at the deadline"
            );
            send(reply, exec, id, Frame::Err { id, error: timed_out() }).await;
        }
        Outcome::Done(Err(err)) => {
            send(reply, exec, id, Frame::Err { id, error: render(err, exec) }).await;
        }
        Outcome::Expired(Some(Err(err))) => {
            send(reply, exec, id, Frame::Err { id, error: body_of(&CallError::from_boxed(err)) }).await;
        }
        Outcome::Expired(None) => {
            send(reply, exec, id, Frame::Err { id, error: timed_out() }).await;
        }
        Outcome::Cancelled(reason) => cancelled(reply, exec, id, reason, halted).await,
    }
}

/// A streamed reply: every `Ok` item as `item`, the clean end as `end`. An `Err` item runs the
/// error handlers on the late path and is written as `err`, which ends the stream, unless they
/// answer `EndStream`. The stream is tracked once the link is known to carry it, and dropped once
/// its end is written, which reports it. On a link that carries no streamed reply the call is
/// answered `err` of kind `internal`, and the stream, never the reply, reports nothing.
#[allow(clippy::too_many_arguments)]
async fn stream_reply(
    shared: &Shared,
    handler: Option<&MountedHandler<Rpc>>,
    exec: &ExecutionRef,
    cx: &RpcCx,
    reply: &ReplyPath,
    id: u64,
    stream: BoxStream<'static, Result<Data, BoxError>>,
    deadline: &mut Option<BoxFuture<'static, ()>>,
    halted: Option<&Halted>,
) {
    if !shared.capabilities.shapes.iter().any(|shape| matches!(shape, Shape::ServerStreaming | Shape::Bidi)) {
        drop(stream);
        let error = ErrorBody::new(ErrorKind::Internal, format!("the {} link carries no streamed reply", shared.link), Details::new());
        send(reply, exec, id, Frame::Err { id, error }).await;
        return;
    }
    let mut stream = Tracked::new(stream, exec.clone());
    loop {
        match until_ended(exec, deadline, stream.next()).await {
            Ended::Done(_) if overflowed(halted) => return cancelled(reply, exec, id, cut_short(exec), halted).await,
            Ended::Done(Some(Ok(data))) => {
                passed(exec, deadline).await;
                if !send(reply, exec, id, Frame::Item { id, data }).await {
                    return;
                }
            }
            Ended::Done(Some(Err(err))) => {
                let frame = match late(handler, exec, cx, err).await {
                    Some(error) => Frame::Err { id, error },
                    None => Frame::End { id },
                };
                send(reply, exec, id, frame).await;
                return;
            }
            Ended::Done(None) => {
                send(reply, exec, id, Frame::End { id }).await;
                return;
            }
            Ended::Cancelled(reason) => return cancelled(reply, exec, id, reason, halted).await,
        }
    }
}

/// An item's error through `dispatch_late`: the error to write, or `None` to end the stream
/// cleanly on `EndStream`. With no handler, a miss's recovered stream, it renders as it stands.
async fn late(handler: Option<&MountedHandler<Rpc>>, exec: &ExecutionRef, cx: &RpcCx, err: BoxError) -> Option<ErrorBody> {
    let Some(handler) = handler else {
        return (!err.is::<EndStream>()).then(|| render(err, exec));
    };
    // `dispatch_late` takes the error by value; the original is kept to write when an error
    // handler answers `Ok`, which on the late path is ignored.
    let original = match err.downcast_ref::<CallError>() {
        Some(call) => call.summary(),
        None => CallError::new(ErrorKind::Internal, "internal error"),
    };
    match ulo::dispatch_late(handler, exec, cx, err).await {
        LateOutcome::End => None,
        LateOutcome::Render(err) => Some(render(err, exec)),
        LateOutcome::Ignored => {
            tracing::warn!(
                handler = handler.name(),
                "an error handler answered `Ok` to an error raised after the reply stream began; its reply is dropped and the original error is written"
            );
            Some(body_of(&original))
        }
        _ => Some(body_of(&original)),
    }
}

/// What a cancelled call writes: nothing to a caller that left or cancelled, `timeout` at the
/// deadline, `unavailable` at the drain's end (the shutdown phrase every refusal carries) or an
/// explicit cancel, and `unavailable` with the reason `overflow` for a call cancelled because its
/// streamed request's queue overflowed.
async fn cancelled(reply: &ReplyPath, exec: &ExecutionRef, id: u64, reason: Option<CancelReason>, halted: Option<&Halted>) {
    // Only an unlimited queue has no limit, and it never overflows.
    let overflow = halted.filter(|halted| halted.get()).and_then(Halted::limit);
    let error = match (reason, overflow) {
        (Some(CancelReason::ClientCancelled | CancelReason::Disconnected), _) => return,
        (Some(CancelReason::Deadline), _) => timed_out(),
        (Some(CancelReason::Drain), _) => ErrorBody::new(ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE, Details::new()),
        (Some(CancelReason::Explicit), Some(limit)) => overflow_answer(limit),
        _ => ErrorBody::new(ErrorKind::Unavailable, "the call was cancelled", Details::new()),
    };
    send(reply, exec, id, Frame::Err { id, error }).await;
}

/// Whether the call's streamed request overflowed. The halt wakes the request's reader before the
/// call is cancelled, so a handler can answer from the failed read, or its reply stream yield, in
/// the poll that would have observed the cancellation; that answer is not the call's.
fn overflowed(halted: Option<&Halted>) -> bool {
    halted.is_some_and(Halted::get)
}

/// The cancel reason of a call whose request overflowed: `Explicit`, the overflow's own, unless
/// another reason landed first.
fn cut_short(exec: &ExecutionRef) -> Option<CancelReason> {
    exec.cancel_with(CancelReason::Explicit);
    exec.cancel_reason()
}

/// The answer to a call whose streamed request queued `limit` items its handler had not read.
fn overflow_answer(limit: usize) -> ErrorBody {
    let message = format!(
        "the streamed request had {limit} items waiting for its handler, the server's `max_queued_items`, and the next one ended the call"
    );
    ErrorBody::new(ErrorKind::Unavailable, message, Details::new().with(reason("overflow")))
}

/// Sends one frame; `false` when it did not reach the caller. A frame over the link's limit, or
/// one its codec cannot encode, is replaced by an `err` of kind `internal` under the same `id`;
/// the second is the server's own code failing and is logged at `error`. Any other failure means
/// the caller is gone, and the call is cancelled `Disconnected`.
async fn send(reply: &ReplyPath, exec: &ExecutionRef, id: u64, frame: Frame) -> bool {
    // Every caller runs in the call's task, instrumented with the call's span, so the current span
    // is the call's. Every failure a caller is told of is an `err` frame sent here.
    let call_span = Span::current();
    if let Frame::Err { error, .. } = &frame {
        trace::record_failure(&call_span, error.kind);
    }
    match reply.send(frame).await {
        Ok(()) => true,
        Err(err) if err.is::<FrameTooLarge>() || err.is::<FrameUnencodable>() => {
            let message = if err.is::<FrameUnencodable>() {
                tracing::error!(error = %err, "a reply frame could not be encoded; the call is answered `internal`");
                "the reply could not be encoded"
            } else {
                "the reply is larger than the link carries"
            };
            let error = ErrorBody::new(ErrorKind::Internal, message, Details::new());
            trace::record_failure(&call_span, error.kind);
            if reply.send(Frame::Err { id, error }).await.is_err() {
                exec.cancel_with(CancelReason::Disconnected);
            }
            false
        }
        Err(err) => {
            tracing::debug!(error = %err, "a reply frame could not be sent; the caller is gone");
            exec.cancel_with(CancelReason::Disconnected);
            false
        }
    }
}

/// A refusal written without an execution, over the in-flight limit or once the drain began,
/// sent from a task of its own and counted in [`Settling::refusals`] until it is sent or dropped.
fn refuse(shared: &Arc<Shared>, tasks: &mut TaskSet, reply: ReplyPath, id: u64, error: ErrorBody) {
    shared.settling.modify(|settling| settling.refusals += 1);
    let pending = Refusing(Arc::clone(shared));
    tasks.spawn(shared.turns.track(async move {
        if let Err(err) = reply.send(Frame::Err { id, error }).await {
            tracing::debug!(error = %err, "a refusal could not be sent; the caller is gone");
        }
        drop(pending);
    }));
}

/// One refusal in [`Settling::refusals`], released when its task ends or is aborted.
struct Refusing(Arc<Shared>);

impl Drop for Refusing {
    fn drop(&mut self) {
        self.0.settling.modify(|settling| settling.refusals -= 1);
    }
}

fn refusal(message: &str) -> ErrorBody {
    ErrorBody::new(ErrorKind::Unavailable, message, Details::new())
}

/// How much of an unreadable `deadline-ms` a refusal quotes back.
const QUOTED: usize = 64;

/// A header value as a refusal quotes it back: in quotes, cut to [`QUOTED`] characters and marked
/// `, cut short` past that. The server's `bad_deadline` and the client's `reserved_header` share it.
pub(crate) fn quote(value: &str) -> String {
    let quoted: String = value.chars().take(QUOTED).collect();
    let cut = if quoted.len() < value.len() { ", cut short" } else { "" };
    format!("{quoted:?}{cut}")
}

/// The refusal of a call whose `deadline-ms` the server cannot read. Running it with no deadline
/// would run unbounded work for a caller that asked for a bound, so it does not run at all.
fn bad_deadline(value: &str) -> ErrorBody {
    let message = format!("the call's `deadline-ms` is {}, which is not a whole number of milliseconds from 0 to {}", quote(value), u64::MAX);
    ErrorBody::new(ErrorKind::BadRequest, message, Details::new().with(reason("bad_deadline")))
}

impl ErrorBody {
    fn with(mut self, detail: Detail) -> Self {
        self.details.push(detail);
        self
    }
}

/// An error no handler claimed, as the `err` frame's `e`: `timeout` when the call's deadline
/// passed, otherwise `CallError::from_boxed`'s kind, message and details.
pub(crate) fn render(err: BoxError, exec: &ExecutionRef) -> ErrorBody {
    if exec.cancel_reason() == Some(CancelReason::Deadline) {
        return timed_out();
    }
    body_of(&CallError::from_boxed(err))
}

fn body_of(error: &CallError) -> ErrorBody {
    ErrorBody::new(error.kind(), error.message(), error.details().clone())
}

fn timed_out() -> ErrorBody {
    body_of(&timeout_error())
}

/// The `Timeout` a passed deadline offers the error handlers, and what an unclaimed one writes.
fn timeout_error() -> CallError {
    CallError::new(ErrorKind::Timeout, "the call's deadline passed")
}

/// The `ErrorInfo` detail carrying `reason`, under this transport's domain.
pub(crate) fn reason(reason: &str) -> Detail {
    Detail::ErrorInfo { reason: reason.to_owned(), domain: DOMAIN.to_owned(), metadata: Default::default() }
}

/// `opts` with the context `headers` carry, under `TraceMode::On`. A `tracestate` with no
/// `traceparent` is discarded (W3C Trace Context §4.2), so headers without one carry none.
fn traced(shared: &Shared, headers: &CallHeaders, opts: ExecOptions) -> ExecOptions {
    if shared.app.trace_mode() == TraceMode::On
        && headers.iter().any(|(name, _)| name.eq_ignore_ascii_case(trace::TRACEPARENT))
        && let Some(ctx) = trace::extract(headers)
    {
        return opts.trace(ctx);
    }
    opts
}

/// A request's span is a server's; an event's handler is a consumer of the emitter's span, as
/// OpenTelemetry's messaging conventions model it (integrations §6.2).
fn call_span(shared: &Shared, exec: &Execution, arrived: Arrived, pattern: &str, handler: Option<&MountedHandler<Rpc>>) -> Span {
    let name = handler.map(|handler| format!("{}::{}", handler.controller(), handler.name()));
    let kind = if arrived == Arrived::Evt { span::Kind::Consumer } else { span::Kind::Server };
    let call_span = span::call(&shared.app, exec, kind, <Rpc as Transport>::KEY, pattern, name.as_deref());
    call_span.record(span::RPC_SYSTEM, "ulo");
    call_span.record(span::RPC_METHOD, pattern);
    // TCP and UDP record none: `messaging.system` names a broker.
    if shared.capabilities.delivery != DeliveryMode::Addressed {
        call_span.record(span::MESSAGING_SYSTEM, shared.link);
    }
    call_span
}

/// A call's registration, removed when the call ends.
struct Registered {
    shared: Arc<Shared>,
    id: u64,
    serial: u64,
}

impl Drop for Registered {
    fn drop(&mut self) {
        let mut calls = self.shared.calls();
        if calls.get(&self.id).is_some_and(|live| live.serial == self.serial) {
            calls.remove(&self.id);
        }
    }
}

/// How a raced future ended: done, or the execution cancelled first, its deadline included.
enum Ended<T> {
    Done(T),
    Cancelled(Option<CancelReason>),
}

/// `fut` raced against the execution's cancellation and its deadline, polled in that order:
/// `fut`, the cancellation, the deadline. An answer ready in the poll the cancellation wakes is
/// the call's answer, a handler that answers on observing its cancellation included, and is
/// written whatever the reason: a caller that cancelled discards it. An execution already
/// cancelled when the race starts ends it at once, so a stream whose items are always ready is
/// cut at its next item rather than run on. A deadline that passes cancels the execution
/// `Deadline`, so every later race ends at its start, and the expired sleep is never polled again.
///
/// The cancellation is written inside the expiry branch's own future, before the race drops
/// `fut`: a handler's future dropped at the deadline reads `Deadline` from its execution.
async fn until_ended<F: Future>(exec: &ExecutionRef, deadline: &mut Option<BoxFuture<'static, ()>>, fut: F) -> Ended<F::Output> {
    if exec.is_cancelled() {
        return Ended::Cancelled(exec.cancel_reason());
    }
    let expired = async {
        expiry(deadline).await;
        exec.cancel_with(CancelReason::Deadline);
    };
    match select(pin!(fut), select(pin!(exec.cancelled()), pin!(expired))).await {
        Either::Left((out, _)) => Ended::Done(out),
        Either::Right((Either::Left(_), _)) => Ended::Cancelled(exec.cancel_reason()),
        Either::Right((Either::Right(_), _)) => Ended::Cancelled(Some(CancelReason::Deadline)),
    }
}

/// Polls the deadline once without waiting, after an item the race took: a deadline passed by
/// then cancels the execution `Deadline`, so the next race ends at its start. The race polls the
/// deadline only while its stream waits, and a stream whose items are always ready never waits.
async fn passed(exec: &ExecutionRef, deadline: &mut Option<BoxFuture<'static, ()>>) {
    let Some(sleep) = deadline else { return };
    if poll_fn(|cx| Poll::Ready(sleep.as_mut().poll(cx).is_ready())).await {
        exec.cancel_with(CancelReason::Deadline);
    }
}

async fn expiry(deadline: &mut Option<BoxFuture<'static, ()>>) {
    match deadline {
        Some(sleep) => sleep.await,
        None => std::future::pending().await,
    }
}
