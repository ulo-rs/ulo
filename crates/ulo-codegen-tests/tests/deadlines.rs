//! A caller's `grpc-timeout` as the call's deadline: DEADLINE_EXCEEDED when it passes unclaimed,
//! the error handlers offered a `Timeout` under `Server::timeout_grace` before that, and
//! DEADLINE_EXCEEDED without them once the grace runs out; the handler's future dropped with the
//! reason `Deadline`; an error handler's single reply delivered and its stream ended with
//! DEADLINE_EXCEEDED, nothing of it written; a deadline passing during a streamed reply; and a
//! `grpc-timeout` off the specification's grammar refused INTERNAL, and a zero one answered
//! DEADLINE_EXCEEDED, before any handler runs, as grpc-go answers each.
//!
//! The calls go through a plain HTTP/2 client: a tonic `Channel` enforces `grpc-timeout` itself
//! and would answer CANCELLED before the server's answer arrived.

mod support;

use std::time::{Duration, Instant};

use futures_util::{Stream, StreamExt, future, stream};
use http::{HeaderMap, Request};
use http_body::Frame;
use http_body_util::{BodyExt, Full, StreamBody};
use prost::Message as _;
use prost::bytes::Bytes;
use tonic::{Code, Status};
use ulo::{BoxError, Bound, Dep, ErrorHandler, ExecutionRef, Interceptor, Module, ModuleDef, ModuleIdentity, Next, injectable, routes};
use ulo_codegen_tests::probe::clock_client::ClockClient;
use ulo_codegen_tests::probe::{self, Tick, Ticks};
use ulo_grpc::{Grpc, GrpcCx, Message, Reply};
use ulo_transport::{CallError, ErrorKind};

use support::{Capture, PlainHttp2, Record, Running};

/// The deadline the stalling calls are given, and the grace the server gives their error handlers.
const DEADLINE: Duration = Duration::from_millis(200);
const GRACE: Duration = Duration::from_millis(300);

/// Longer than any test runs: a handler sleeping this long is still running at the deadline.
const FOREVER: Duration = Duration::from_secs(30);

/// How long `Late` holds its thread, and the message it answers with.
const LATE: Duration = Duration::from_millis(300);
const LATE_TICK: u32 = 42;

const STREAM_WARNING: &str = "an error handler answered a timed-out call with a stream; the stream was ended at the deadline";

/// What the handlers and error handlers observed, in order.
#[derive(Clone)]
struct Seen(Record<String>);

/// Records the execution's cancel reason when the handler's future holding it is dropped.
struct DropProbe {
    exec: ExecutionRef,
    seen: Seen,
}

impl Drop for DropProbe {
    fn drop(&mut self) {
        self.seen.0.push(format!("handler dropped: {:?}", self.exec.cancel_reason()));
    }
}

/// The kind of the error it is offered, recorded, and every error claimed with UNAVAILABLE.
#[injectable]
struct Claim {
    seen: Dep<Seen>,
}

impl ErrorHandler<Grpc> for Claim {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        let kind = err.downcast_ref::<CallError>().map(CallError::kind);
        self.seen.0.push(format!("offered: {kind:?}"));
        Ok(cx.reply_status(Status::unavailable("claimed after the deadline")))
    }
}

/// Reshapes every error into an `Unavailable` `CallError`.
struct Reshape;

impl ErrorHandler<Grpc> for Reshape {
    async fn handle(&self, _err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        Err(BoxError::from(CallError::new(ErrorKind::Unavailable, "reshaped")))
    }
}

/// Takes longer than any grace to answer.
#[injectable]
struct Stuck {
    seen: Dep<Seen>,
}

impl ErrorHandler<Grpc> for Stuck {
    async fn handle(&self, err: BoxError, _cx: &GrpcCx) -> Result<Reply, BoxError> {
        self.seen.0.push("stuck handler started".to_owned());
        tokio::time::sleep(FOREVER).await;
        Err(err)
    }
}

fn timed_out(err: &BoxError) -> bool {
    err.downcast_ref::<CallError>().is_some_and(|call| call.kind() == ErrorKind::Timeout)
}

/// One `Tick { n: 7 }`.
struct OneOnTimeout;

impl ErrorHandler<Grpc> for OneOnTimeout {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply(Tick { n: 7 }))
    }
}

/// Ticks without end.
struct EndlessOnTimeout;

impl ErrorHandler<Grpc> for EndlessOnTimeout {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::repeat_with(|| Ok::<_, CallError>(Tick { n: 1 }))))
    }
}

/// One tick, then nothing.
struct TrickleOnTimeout;

impl ErrorHandler<Grpc> for TrickleOnTimeout {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<Reply, BoxError> {
        if !timed_out(&err) {
            return Err(err);
        }
        Ok(cx.reply_stream(stream::once(future::ready(Ok::<_, CallError>(Tick { n: 1 }))).chain(stream::pending())))
    }
}

#[injectable]
/// Answers with a body of its own and never runs the handler: a reply no `encode_stream` built
/// carries no `Streamed` mark, so the outermost body treats it as one that may be unary.
struct Unmarked;

impl Interceptor<Grpc> for Unmarked {
    async fn intercept(&self, _cx: &GrpcCx, _next: Next<'_, Grpc>) -> Result<Reply, BoxError> {
        Ok(Reply::new(tonic::body::Body::new(burst())))
    }
}

/// How many messages `Burst` writes before its OK trailers.
const BURST: u32 = 6;

/// Which message of `burst` holds its thread past the deadline before it is written.
const BURST_SLOW: u32 = 1;

/// `BURST` messages numbered from 1, one frame each, then OK trailers. Frame `BURST_SLOW` yields
/// once, so the deadline's timer is polled, then holds its thread past the deadline; every frame
/// after it is ready at once, so the poll after it finds the deadline passed with the next message
/// ready.
fn burst() -> impl http_body::Body<Data = Bytes, Error = Status> + Send + 'static {
    StreamBody::new(stream::unfold(0u32, |sent| async move {
        if sent > BURST {
            return None;
        }
        if sent == BURST_SLOW {
            tokio::task::yield_now().await;
            std::thread::sleep(LATE);
        }
        let frame = if sent < BURST {
            let message = Tick { n: sent + 1 }.encode_to_vec();
            let mut framed = vec![0];
            framed.extend_from_slice(&u32::try_from(message.len()).unwrap_or_default().to_be_bytes());
            framed.extend_from_slice(&message);
            Frame::data(Bytes::from(framed))
        } else {
            let mut trailers = HeaderMap::new();
            trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
            Frame::trailers(trailers)
        };
        Some((Ok::<_, Status>(frame), sent + 1))
    }))
}

#[injectable]
struct ClockService {
    seen: Dep<Seen>,
}

impl ClockService {
    async fn stall(&self, exec: ExecutionRef) -> Tick {
        let _probe = DropProbe { exec, seen: (*self.seen).clone() };
        tokio::time::sleep(FOREVER).await;
        Tick { n: 0 }
    }
}

#[routes]
impl ClockService {
    #[ulo_grpc::method(probe::clock::Stall)]
    async fn stall_unclaimed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallClaimed)]
    #[error_handlers(Claim)]
    async fn stall_claimed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallReshaped)]
    #[error_handlers(value = Reshape)]
    async fn stall_reshaped(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallStuck)]
    #[error_handlers(Stuck)]
    async fn stall_stuck(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    /// The time left before the call's deadline, in whole milliseconds rounded up; `0` for a call
    /// without one.
    #[ulo_grpc::method(probe::clock::Quick)]
    fn quick(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.seen.0.push("quick ran".to_owned());
        let left = exec.deadline().map_or(0, |deadline| {
            let left = deadline.saturating_duration_since(tokio::time::Instant::now().into_std());
            u32::try_from(left.as_millis()).unwrap_or(u32::MAX).saturating_add(1)
        });
        Tick { n: left }
    }

    #[ulo_grpc::method(probe::clock::StallAnswered)]
    #[error_handlers(value = OneOnTimeout)]
    async fn stall_answered(&self, _req: Message<Ticks>, exec: ExecutionRef) -> Tick {
        self.stall(exec).await
    }

    #[ulo_grpc::method(probe::clock::StallStreamed)]
    #[error_handlers(value = EndlessOnTimeout)]
    async fn stall_streamed(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        let _ = self.stall(exec).await;
        stream::empty()
    }

    #[ulo_grpc::method(probe::clock::StallTrickled)]
    #[error_handlers(value = TrickleOnTimeout)]
    async fn stall_trickled(&self, _req: Message<Ticks>, exec: ExecutionRef) -> impl Stream<Item = Result<Tick, CallError>> {
        let _ = self.stall(exec).await;
        stream::empty()
    }

    /// Yields once, so the dispatcher polls the deadline's timer, then holds its thread past the
    /// deadline and answers: the reply is ready in the poll the deadline passes in.
    #[ulo_grpc::method(probe::clock::Late)]
    async fn late(&self, _req: Message<Ticks>) -> Tick {
        tokio::task::yield_now().await;
        std::thread::sleep(LATE);
        Tick { n: LATE_TICK }
    }

    /// A reply stream that yields nothing: its body is waiting for a message at the deadline.
    #[ulo_grpc::method(probe::clock::Hush)]
    fn hush(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> {
        stream::pending()
    }

    #[ulo_grpc::method(probe::clock::Burst)]
    #[interceptors(value = Unmarked)]
    fn burst(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> {
        stream::empty()
    }

    /// One tick, then nothing until the reply is ended.
    #[ulo_grpc::method(probe::clock::Drip)]
    fn drip(&self, _req: Message<Ticks>) -> impl Stream<Item = Result<Tick, CallError>> {
        self.seen.0.push("drip ran".to_owned());
        stream::once(async { Ok(Tick { n: 1 }) }).chain(stream::pending())
    }
}

struct Root {
    seen: Seen,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.seen.clone());
        m.provide::<Claim>();
        m.provide::<Stuck>();
        m.controller::<ClockService>();
    }
}

async fn start() -> (Running, ClockClient<PlainHttp2>, Seen) {
    let seen = Seen(Record::new());
    let server = ulo_grpc::Server::new("127.0.0.1:0").timeout_grace(Bound::After(GRACE));
    let app = Running::start(Root { seen: seen.clone() }, server).await;
    let (http2, origin) = app.plain_http2();
    (app, ClockClient::with_origin(http2, origin), seen)
}

fn timed(timeout: Duration) -> tonic::Request<Ticks> {
    let mut request = tonic::Request::new(Ticks::default());
    request.set_timeout(timeout);
    request
}

fn failed<T: std::fmt::Debug>(reply: Result<tonic::Response<T>, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => panic!("expected the call to fail, got {:?}", reply.into_inner()),
    }
}

#[tokio::test]
async fn an_unclaimed_deadline_answers_deadline_exceeded_and_drops_the_handler_as_deadline() {
    let (app, mut client, seen) = start().await;
    let status = failed(support::within("the stalled call", client.stall(timed(DEADLINE))).await);
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert_eq!(seen.0.at_least(1, "the handler's drop").await, vec!["handler dropped: Some(Deadline)".to_owned()]);
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_claiming_the_timeout_within_the_grace_is_the_reply() {
    let (app, mut client, seen) = start().await;
    let status = failed(support::within("the stalled call", client.stall_claimed(timed(DEADLINE))).await);
    assert_eq!((status.code(), status.message()), (Code::Unavailable, "claimed after the deadline"));
    let observed = seen.0.at_least(2, "the drop and the error handler").await;
    assert_eq!(observed, vec!["handler dropped: Some(Deadline)".to_owned(), "offered: Some(Timeout)".to_owned()]);
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_reshaping_the_timeout_is_rendered_as_it_stands() {
    let (app, mut client, _) = start().await;
    let status = failed(support::within("the stalled call", client.stall_reshaped(timed(DEADLINE))).await);
    assert_eq!((status.code(), status.message()), (Code::Unavailable, "reshaped"));
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_past_the_grace_is_dropped_for_deadline_exceeded() {
    let (app, mut client, seen) = start().await;
    let began = Instant::now();
    let status = failed(support::within("the stalled call", client.stall_stuck(timed(DEADLINE))).await);
    let took = began.elapsed();
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(seen.0.snapshot().contains(&"stuck handler started".to_owned()), "the error handler was never offered the timeout");
    assert!(took >= DEADLINE + GRACE, "answered after {took:?}, before the deadline and the grace had passed");
    assert!(took < DEADLINE + GRACE + Duration::from_secs(1), "answered after {took:?}: the grace did not bound the error handler");
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_answering_one_message_within_the_grace_is_the_reply() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, mut client, _) = start().await;
    let reply = support::within("the stalled call", client.stall_answered(timed(DEADLINE))).await;
    assert_eq!(reply.map(|reply| reply.into_inner().n).map_err(|status| status.code()), Ok(7));
    let warned = capture.warnings();
    assert!(!warned.iter().any(|line| line.contains(STREAM_WARNING)), "a single reply was logged as a stream: {warned:?}");
    app.stop().await;
}

/// The failure a server-streaming call ended with: its status when the call failed outright, or
/// the panic naming what the stream carried instead.
async fn stream_failed(reply: Result<tonic::Response<tonic::Streaming<Tick>>, Status>) -> Status {
    match reply {
        Err(status) => status,
        Ok(reply) => {
            let first = support::within("the first item", reply.into_inner().next()).await;
            panic!("expected the call to fail before any item, got a stream whose first item is {first:?}")
        }
    }
}

#[tokio::test]
async fn an_error_handler_answering_a_stream_is_ended_at_once_with_deadline_exceeded() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, mut client, _) = start().await;
    let began = Instant::now();
    let reply = support::within("the stalled call", client.stall_streamed(timed(DEADLINE))).await;
    let status = stream_failed(reply).await;
    let took = began.elapsed();
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(took < DEADLINE + GRACE, "answered after {took:?}, not at once after the {DEADLINE:?} deadline");
    let warned = capture.warnings();
    assert!(
        warned.iter().any(|line| line.contains(STREAM_WARNING) && line.contains("path=/codegen.probe.v1.Clock/StallStreamed")),
        "the server logged no warning naming the path: {warned:?}"
    );
    app.stop().await;
}

#[tokio::test]
async fn an_error_handler_reply_still_open_at_the_end_of_the_grace_is_ended_with_deadline_exceeded() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, mut client, _) = start().await;
    let began = Instant::now();
    let reply = support::within("the stalled call", client.stall_trickled(timed(DEADLINE))).await;
    let status = stream_failed(reply).await;
    let took = began.elapsed();
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(took >= DEADLINE + GRACE, "answered after {took:?}, before the deadline and the grace had passed");
    assert!(took < DEADLINE + GRACE + Duration::from_secs(1), "answered after {took:?}: the grace did not bound the reply");
    let warned = capture.warnings();
    assert!(warned.iter().any(|line| line.contains(STREAM_WARNING)), "the server logged no warning: {warned:?}");
    app.stop().await;
}

#[tokio::test]
async fn a_call_answered_before_its_deadline_reads_the_deadline_it_was_given() {
    let (app, mut client, _) = start().await;
    let reply = client.quick(timed(Duration::from_secs(10))).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    let left = reply.into_inner().n;
    assert!((1..=10_000).contains(&left), "{left} ms left of a 10 s deadline");
    let reply = client.quick(Ticks::default()).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert_eq!(reply.into_inner().n, 0, "a call without `grpc-timeout` has no deadline");
    app.stop().await;
}

/// `request` with `grpc-timeout: value` written as raw metadata, as a caller off the grammar sends
/// it; tonic's own `set_timeout` writes only values on it.
fn raw_timeout<T>(mut request: tonic::Request<T>, value: &str) -> tonic::Request<T> {
    request.metadata_mut().insert("grpc-timeout", value.parse().unwrap_or_else(|error| panic!("bad header {value:?}: {error}")));
    request
}

/// Asserts a call carrying `grpc-timeout: value` was refused as off the grammar: INTERNAL, the
/// message quoting `quoted`.
fn off_the_grammar(status: &Status, value: &str, quoted: &str) {
    assert_eq!(status.code(), Code::Internal, "`grpc-timeout: {value:?}`: {status:?}");
    assert!(status.message().contains(&format!("`grpc-timeout` is {quoted}")), "`grpc-timeout: {value:?}`: the message does not quote it: {status:?}");
}

/// F396: a unary call and a server-streaming call whose `grpc-timeout` is `value`, off the
/// grammar, are each refused INTERNAL before any handler runs.
async fn refused_before_the_handler_runs(value: &str) {
    let (app, mut client, seen) = start().await;
    let unary = failed(support::within("the unary call", client.quick(raw_timeout(tonic::Request::new(Ticks::default()), value))).await);
    off_the_grammar(&unary, value, &format!("{value:?}"));
    let streamed = stream_failed(support::within("the streaming call", client.drip(raw_timeout(tonic::Request::new(Ticks::default()), value))).await).await;
    off_the_grammar(&streamed, value, &format!("{value:?}"));
    // A call on the grammar, answered, shows the server had settled everything before it.
    let reply = client.quick(Ticks::default()).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert_eq!(reply.into_inner().n, 0);
    assert_eq!(seen.0.snapshot(), vec!["quick ran".to_owned()], "the handlers that ran: only the call without `grpc-timeout`'s");
    app.stop().await;
}

#[tokio::test]
async fn an_overflowing_grpc_timeout_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("18446744073709551616S").await;
}

#[tokio::test]
async fn a_grpc_timeout_that_is_not_a_number_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("abcS").await;
}

#[tokio::test]
async fn a_grpc_timeout_without_a_unit_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("10").await;
}

#[tokio::test]
async fn a_grpc_timeout_with_an_unknown_unit_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("10x").await;
    refused_before_the_handler_runs("10s").await;
}

#[tokio::test]
async fn a_nine_digit_grpc_timeout_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("123456789S").await;
}

#[tokio::test]
async fn an_empty_grpc_timeout_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("").await;
}

#[tokio::test]
async fn a_negative_grpc_timeout_is_refused_before_the_handler_runs() {
    refused_before_the_handler_runs("-1S").await;
}

/// The refusal quotes the value whole up to 64 characters, cut to 64 and marked past that.
#[tokio::test]
async fn the_grpc_timeout_refusal_quotes_the_value_cut_short() {
    let (app, mut client, _) = start().await;
    let long = format!("{}S", "9".repeat(100));
    let status = failed(support::within("the call", client.quick(raw_timeout(tonic::Request::new(Ticks::default()), &long))).await);
    off_the_grammar(&status, &long, &format!("{:?}, cut short", "9".repeat(64)));
    app.stop().await;
}

/// A value on the grammar still sets the deadline, eight digits of hours the longest: `5S` and
/// `99999999H` are answered with the deadline they give, and `50m` ends a stalled call at it.
#[tokio::test]
async fn a_grpc_timeout_on_the_grammar_still_sets_the_deadline() {
    let (app, mut client, seen) = start().await;
    let reply = client.quick(raw_timeout(tonic::Request::new(Ticks::default()), "5S")).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    let left = reply.into_inner().n;
    assert!((4_000..=5_000).contains(&left), "{left} ms left of `grpc-timeout: 5S`");
    let reply = client.quick(raw_timeout(tonic::Request::new(Ticks::default()), "99999999H")).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert_eq!(reply.into_inner().n, u32::MAX, "the milliseconds left of `grpc-timeout: 99999999H`, held to a `u32`");
    let began = Instant::now();
    let status = failed(support::within("the stalled call", client.stall(raw_timeout(tonic::Request::new(Ticks::default()), "50m"))).await);
    assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}");
    assert!(began.elapsed() < Duration::from_secs(5), "the 50 ms deadline ended the call after {:?}", began.elapsed());
    assert!(seen.0.at_least(3, "the stalled handler's drop").await.contains(&"handler dropped: Some(Deadline)".to_owned()));
    app.stop().await;
}

/// F399: calls whose `grpc-timeout` is `value`, a zero amount, are each answered DEADLINE_EXCEEDED
/// before any handler runs, as grpc-go answers them: a unary handler that answers without awaiting,
/// a streaming one, and one whose error handler would claim a `Timeout` with UNAVAILABLE, which
/// shows no error handler is offered it.
async fn spent_before_the_handler_runs(value: &str) {
    let (app, mut client, seen) = start().await;
    let unary = failed(support::within("the unary call", client.quick(raw_timeout(tonic::Request::new(Ticks::default()), value))).await);
    assert_eq!(unary.code(), Code::DeadlineExceeded, "`grpc-timeout: {value:?}`: {unary:?}");
    let streamed = stream_failed(support::within("the streaming call", client.drip(raw_timeout(tonic::Request::new(Ticks::default()), value))).await).await;
    assert_eq!(streamed.code(), Code::DeadlineExceeded, "`grpc-timeout: {value:?}`: {streamed:?}");
    let claimed = failed(support::within("the claimed call", client.stall_claimed(raw_timeout(tonic::Request::new(Ticks::default()), value))).await);
    assert_eq!(claimed.code(), Code::DeadlineExceeded, "`grpc-timeout: {value:?}`: {claimed:?}");
    // A call without the header, answered, shows the server had settled everything before it.
    let reply = client.quick(Ticks::default()).await.unwrap_or_else(|status| panic!("Quick failed: {status:?}"));
    assert_eq!(reply.into_inner().n, 0);
    assert_eq!(seen.0.snapshot(), vec!["quick ran".to_owned()], "`grpc-timeout: {value:?}`: only the call without it ran a handler");
    app.stop().await;
}

#[tokio::test]
async fn a_zero_grpc_timeout_is_answered_deadline_exceeded_before_the_handler_runs() {
    spent_before_the_handler_runs("0S").await;
}

#[tokio::test]
async fn a_zero_grpc_timeout_in_any_unit_is_answered_deadline_exceeded_before_the_handler_runs() {
    for value in ["0H", "0M", "0m", "0u", "0n", "00000000S"] {
        spent_before_the_handler_runs(value).await;
    }
}

#[tokio::test]
async fn a_deadline_passing_during_a_streamed_reply_ends_it_with_deadline_exceeded() {
    let (app, mut client, _) = start().await;
    let reply = client.drip(timed(Duration::from_millis(300))).await.unwrap_or_else(|status| panic!("Drip failed: {status:?}"));
    let mut items = reply.into_inner();
    let first = support::within("the first tick", items.next()).await;
    assert_eq!(first.map(|item| item.map(|tick| tick.n).map_err(|status| status.code())), Some(Ok(1)));
    let end = support::within("the reply's end", items.next()).await;
    match end {
        Some(Err(status)) => assert_eq!(status.code(), Code::DeadlineExceeded, "{status:?}"),
        other => panic!("expected the reply to end with DEADLINE_EXCEEDED, got {other:?}"),
    }
    app.stop().await;
}

/// One frame of a raw reply body.
#[derive(Debug)]
enum Written {
    Message(Tick),
    Trailers(HeaderMap),
}

/// `path` called over the plain client with `grpc-timeout: timeout` and an empty `Ticks`, the
/// reply's head and every frame of its body, in order, to the body's end.
async fn raw_call(app: &Running, client: &PlainHttp2, path: &str, timeout: &str) -> (HeaderMap, Vec<Written>) {
    let request = Request::post(format!("{}{path}", app.origin()))
        .header("content-type", "application/grpc")
        .header("te", "trailers")
        .header("grpc-timeout", timeout)
        .body(tonic::body::Body::new(Full::new(Bytes::from_static(&[0; 5]))))
        .unwrap_or_else(|error| panic!("bad request: {error}"));
    let reply = support::within("the reply's head", client.request(request)).await.unwrap_or_else(|error| panic!("{path} failed: {error}"));
    let (head, mut body) = reply.into_parts();
    let mut written = Vec::new();
    while let Some(frame) = support::within("the reply's next frame", body.frame()).await {
        let frame = frame.unwrap_or_else(|error| panic!("the reply's body failed: {error}"));
        match frame.into_data() {
            Ok(data) => {
                assert_eq!(data.first(), Some(&0), "a compressed or malformed message: {data:?}");
                let tick = Tick::decode(&data[5..]).unwrap_or_else(|error| panic!("an undecodable message: {error}"));
                written.push(Written::Message(tick));
            }
            Err(frame) => written.push(Written::Trailers(frame.into_trailers().unwrap_or_else(|_| panic!("a frame of neither kind")))),
        }
    }
    (head.headers, written)
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// A worker pool, so the deadline's timer fires on a thread other than the one `Late` holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_unary_reply_ready_in_the_deadlines_poll_is_sent_with_ok_trailers() {
    let (app, _client, _) = start().await;
    let (plain, _) = app.plain_http2();
    let (headers, written) = raw_call(&app, &plain, "/codegen.probe.v1.Clock/Late", "50m").await;
    assert_eq!(header(&headers, "grpc-status"), None, "the head of a reply with a message is not a trailers-only status: {headers:?}");
    match written.as_slice() {
        [Written::Message(tick), Written::Trailers(trailers)] => {
            assert_eq!(tick.n, LATE_TICK);
            assert_eq!(header(trailers, "grpc-status"), Some("0"), "{trailers:?}");
            assert_eq!(header(trailers, "grpc-message"), None, "{trailers:?}");
        }
        other => panic!("expected the message, then OK trailers, and nothing after, got {other:?}"),
    }
    app.stop().await;
}

#[tokio::test]
async fn a_reply_still_waiting_for_its_message_at_the_deadline_is_ended_with_deadline_exceeded_and_no_message() {
    let (app, _client, _) = start().await;
    let (plain, _) = app.plain_http2();
    let (headers, written) = raw_call(&app, &plain, "/codegen.probe.v1.Clock/Hush", "200m").await;
    let status = match written.as_slice() {
        [Written::Trailers(trailers)] => trailers,
        [] => &headers,
        other => panic!("expected no message and one set of trailers, got {other:?}"),
    };
    assert_eq!(header(status, "grpc-status"), Some("4"), "{status:?}");
    assert_eq!(header(status, "grpc-message"), Some("the%20call's%20deadline%20passed"), "grpc-message is percent-encoded: {status:?}");
    app.stop().await;
}

/// The health service's `Watch` is a builtin reply stream, which no handler marks: it reads no
/// cancellation, so the outermost body is what ends it when the deadline passes while it waits
/// for a change, with DEADLINE_EXCEEDED trailers after the status it sent first.
#[tokio::test]
async fn an_unmarked_reply_stream_waiting_at_the_deadline_is_ended_with_deadline_exceeded() {
    let (app, _client, _) = start().await;
    let (plain, _) = app.plain_http2();
    let (headers, written) = raw_call(&app, &plain, "/grpc.health.v1.Health/Watch", "200m").await;
    assert_eq!(header(&headers, "grpc-status"), None, "the head of a reply that sent a message is not a trailers-only status: {headers:?}");
    match written.as_slice() {
        [Written::Message(_), Written::Trailers(trailers)] => {
            assert_eq!(header(trailers, "grpc-status"), Some("4"), "{trailers:?}");
            assert_eq!(header(trailers, "grpc-message"), Some("the%20call's%20deadline%20passed"), "{trailers:?}");
        }
        other => panic!("expected the first status, then DEADLINE_EXCEEDED trailers, got {other:?}"),
    }
    app.stop().await;
}

/// A reply that is not a marked stream may be a unary one, whose message has been written and
/// which ends with its own trailers; a second data frame after the deadline is what tells it
/// from one, and it is not taken. `Burst`'s frame `BURST_SLOW` is written, the poll after it
/// finds the deadline passed with the next message ready, that message goes out with the poll,
/// and the one after it is dropped for DEADLINE_EXCEEDED. A worker pool, so the deadline's timer
/// fires on a thread other than the one the slow frame holds.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unmarked_body_yielding_a_second_data_frame_after_the_deadline_is_ended_without_it() {
    let (app, _client, _) = start().await;
    let (plain, _) = app.plain_http2();
    let (headers, written) = raw_call(&app, &plain, "/codegen.probe.v1.Clock/Burst", "50m").await;
    assert_eq!(header(&headers, "grpc-status"), None, "{headers:?}");
    let Some((Written::Trailers(trailers), messages)) = written.split_last() else {
        panic!("expected messages and trailers, got {written:?}");
    };
    assert_eq!(header(trailers, "grpc-status"), Some("4"), "the body ran to its own OK trailers: {written:?}");
    let numbers: Vec<u32> = messages
        .iter()
        .map(|frame| match frame {
            Written::Message(tick) => tick.n,
            Written::Trailers(other) => panic!("trailers before the end: {other:?}"),
        })
        .collect();
    assert_eq!(numbers, vec![1, 2, 3], "the messages written before the deadline and the one in its poll, and no other");
    app.stop().await;
}
