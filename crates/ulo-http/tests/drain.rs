//! A call still running when the drain window ends is abandoned: its execution is cancelled with
//! `CancelReason::Drain` and the call is answered with the one shutdown refusal. A response not yet
//! begun is 503 with `Connection: close` and the problem document the other refusals use; one
//! begun is a body that errors, which a backend writes as a closed connection (HTTP/1.1) or a reset
//! stream (HTTP/2), never as the terminating chunk or `END_STREAM`. A call that finishes inside the
//! window is untouched, and so is an answer the handler chose on observing the cancellation,
//! unless that answer is a stream, which would be cut before its first frame and is answered 503.
//! A begun body's frame ready in the poll the cancellation wakes is written before the error
//! frame, and no frame is taken in a later poll.

mod o1_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures_util::{StreamExt, stream};
use http_body_util::BodyExt;
use o1_support::{Call, PATIENCE, read, start_draining};
use tokio::sync::{Notify, Semaphore};
use ulo::{BoxError, CancelReason, Dep, ExecutionRef, ModuleDef, StreamOutcome, injectable, routes};
use ulo_http::{Bytes, HttpBody, Response, Routing, StatusCode};
use ulo_transport::{CallError, ErrorKind};

/// How many frames the six-frame bodies have; each would end after its last.
const SIX: usize = 6;
/// The frame `/self-drained` cancels its execution on.
const CANCELLED_AT: usize = 3;

/// The window the tests let a held call outlive.
const WINDOW: Duration = Duration::from_millis(300);

/// What the held handlers share with the test.
struct Hold {
    release: Semaphore,
    entered: Notify,
    /// The cancel reason of each held handler's future as it dropped.
    dropped: Mutex<Vec<Option<CancelReason>>>,
    ends: Mutex<Vec<StreamOutcome>>,
    entries: AtomicUsize,
    /// How many frames the six-frame bodies below produced.
    produced: Arc<AtomicUsize>,
    /// The gated body's second frame waits here, and `waiting` is notified as it starts to.
    gate: Arc<Notify>,
    waiting: Arc<Notify>,
    /// The gated call's execution, for the test to cancel.
    exec: Mutex<Option<ExecutionRef>>,
}

impl Default for Hold {
    fn default() -> Self {
        Hold {
            release: Semaphore::new(0),
            entered: Notify::new(),
            dropped: Mutex::new(Vec::new()),
            ends: Mutex::new(Vec::new()),
            entries: AtomicUsize::new(0),
            produced: Arc::new(AtomicUsize::new(0)),
            gate: Arc::new(Notify::new()),
            waiting: Arc::new(Notify::new()),
            exec: Mutex::new(None),
        }
    }
}

struct OnDrop<'a>(&'a Hold, ExecutionRef);

impl Drop for OnDrop<'_> {
    fn drop(&mut self) {
        self.0.dropped.lock().unwrap_or_else(PoisonError::into_inner).push(self.1.cancel_reason());
    }
}

impl Hold {
    async fn wait(&self) {
        self.entries.fetch_add(1, Ordering::SeqCst);
        self.entered.notify_one();
        self.release.acquire().await.expect("the semaphore is never closed").forget();
    }

    fn dropped(&self) -> Vec<Option<CancelReason>> {
        self.dropped.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn ends(&self) -> Vec<StreamOutcome> {
        self.ends.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[injectable]
struct Held {
    hold: Dep<Hold>,
}

#[routes]
impl Held {
    /// Answers once released.
    #[ulo_http::get("/hold")]
    async fn hold(&self, exec: ExecutionRef) -> &'static str {
        let _dropped = OnDrop(&self.hold, exec);
        self.hold.wait().await;
        "released"
    }

    /// As `/hold`, its execution cancelled by its own handler once the drain has begun.
    #[ulo_http::get("/cancelled-hold")]
    async fn cancelled_hold(&self, exec: ExecutionRef) -> &'static str {
        let _dropped = OnDrop(&self.hold, exec.clone());
        self.hold.entered.notify_one();
        exec.draining().await;
        exec.cancel();
        self.hold.release.acquire().await.expect("the semaphore is never closed").forget();
        "released"
    }

    /// As `/cancelled-hold`, answering with a stream.
    #[ulo_http::get("/cancelled-then-streams")]
    async fn cancelled_then_streams(&self, exec: ExecutionRef) -> Response {
        let _dropped = OnDrop(&self.hold, exec.clone());
        self.hold.entered.notify_one();
        exec.draining().await;
        exec.cancel();
        self.hold.release.acquire().await.expect("the semaphore is never closed").forget();
        Response::new(HttpBody::stream(stream::iter([Ok::<_, std::convert::Infallible>(Bytes::from_static(b"released"))])))
    }

    /// Answers once its execution is cancelled, as a handler watching for it does.
    #[ulo_http::get("/answers-when-cancelled")]
    async fn answers_when_cancelled(&self, exec: ExecutionRef) -> &'static str {
        let _dropped = OnDrop(&self.hold, exec.clone());
        self.hold.entered.notify_one();
        exec.cancelled().await;
        "answered after the cancellation"
    }

    /// Answers with a stream once its execution is cancelled.
    #[ulo_http::get("/streams-when-cancelled")]
    async fn streams_when_cancelled(&self, exec: ExecutionRef) -> Response {
        let _dropped = OnDrop(&self.hold, exec.clone());
        let ends = self.hold.clone();
        exec.on_stream_end(move |outcome| ends.ends.lock().unwrap_or_else(PoisonError::into_inner).push(outcome));
        self.hold.entered.notify_one();
        exec.cancelled().await;
        Response::new(HttpBody::stream(stream::iter([Ok::<_, std::convert::Infallible>(Bytes::from_static(b"never written"))])))
    }

    /// A `HEAD` handler answering with a stream once its execution is cancelled: a body the
    /// backend never writes.
    #[ulo_http::head("/head-streams-when-cancelled")]
    async fn head_streams_when_cancelled(&self, exec: ExecutionRef) -> Response {
        let _dropped = OnDrop(&self.hold, exec.clone());
        self.hold.entered.notify_one();
        exec.cancelled().await;
        Response::new(HttpBody::stream(stream::iter([Ok::<_, std::convert::Infallible>(Bytes::from_static(b"discarded"))])))
    }

    /// Answers 204 carrying a stream once its execution is cancelled: a body the backend never
    /// writes.
    #[ulo_http::get("/no-content-when-cancelled")]
    async fn no_content_when_cancelled(&self, exec: ExecutionRef) -> Response {
        let _dropped = OnDrop(&self.hold, exec.clone());
        self.hold.entered.notify_one();
        exec.cancelled().await;
        let mut response = Response::new(HttpBody::stream(stream::iter([Ok::<_, std::convert::Infallible>(Bytes::from_static(b"discarded"))])));
        *response.status_mut() = StatusCode::NO_CONTENT;
        response
    }

    /// Answers at once with a stream: `first`, then `second` once released, then its end.
    #[ulo_http::get("/stream")]
    async fn stream(&self, exec: ExecutionRef) -> Response {
        let hold = self.hold.clone();
        let ends = self.hold.clone();
        exec.on_stream_end(move |outcome| ends.ends.lock().unwrap_or_else(PoisonError::into_inner).push(outcome));
        let body = stream::unfold(0, move |step| {
            let hold = hold.clone();
            async move {
                match step {
                    0 => Some((Ok::<_, std::convert::Infallible>(Bytes::from_static(b"first")), 1)),
                    1 => {
                        hold.wait().await;
                        Some((Ok(Bytes::from_static(b"second")), 2))
                    }
                    _ => None,
                }
            }
        });
        Response::new(HttpBody::stream(body))
    }

    /// Six frames, every one ready, the third cancelling the execution with the drain's reason as
    /// it is produced: the frame and the cancellation are ready in one poll.
    #[ulo_http::get("/self-drained")]
    async fn self_drained(&self, exec: ExecutionRef) -> Response {
        let ends = self.hold.clone();
        exec.on_stream_end(move |outcome| ends.ends.lock().unwrap_or_else(PoisonError::into_inner).push(outcome));
        let produced = Arc::clone(&self.hold.produced);
        let body = stream::iter(1..=SIX).map(move |n| {
            produced.fetch_add(1, Ordering::SeqCst);
            if n == CANCELLED_AT {
                exec.cancel_with(CancelReason::Drain);
            }
            Ok::<_, std::convert::Infallible>(Bytes::from(n.to_string()))
        });
        Response::new(HttpBody::stream(body))
    }

    /// Six frames: the first ready, the second once the gate opens, the rest ready.
    #[ulo_http::get("/gated")]
    async fn gated(&self, exec: ExecutionRef) -> Response {
        let ends = self.hold.clone();
        exec.on_stream_end(move |outcome| ends.ends.lock().unwrap_or_else(PoisonError::into_inner).push(outcome));
        *self.hold.exec.lock().unwrap_or_else(PoisonError::into_inner) = Some(exec);
        let (produced, gate, waiting) = (Arc::clone(&self.hold.produced), Arc::clone(&self.hold.gate), Arc::clone(&self.hold.waiting));
        let body = stream::iter(1..=SIX).then(move |n| {
            let (produced, gate, waiting) = (Arc::clone(&produced), Arc::clone(&gate), Arc::clone(&waiting));
            async move {
                if n == 2 {
                    waiting.notify_one();
                    gate.notified().await;
                }
                produced.fetch_add(1, Ordering::SeqCst);
                Ok::<_, std::convert::Infallible>(Bytes::from(n.to_string()))
            }
        });
        Response::new(HttpBody::stream(body))
    }

    /// As `/stream`, its execution cancelled by its own body once the drain has begun: a reason that
    /// is not the drain's.
    #[ulo_http::get("/cancelled-stream")]
    async fn cancelled_stream(&self, exec: ExecutionRef) -> Response {
        let ends = self.hold.clone();
        exec.on_stream_end(move |outcome| ends.ends.lock().unwrap_or_else(PoisonError::into_inner).push(outcome));
        let hold = self.hold.clone();
        let body = stream::iter([Bytes::from_static(b"first"), Bytes::from_static(b"second")]).then(move |chunk| {
            let (hold, exec) = (hold.clone(), exec.clone());
            async move {
                if chunk == "second" {
                    hold.entered.notify_one();
                    exec.draining().await;
                    exec.cancel();
                    hold.release.acquire().await.expect("the semaphore is never closed").forget();
                }
                Ok::<_, std::convert::Infallible>(chunk)
            }
        });
        Response::new(HttpBody::stream(body))
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.value(Hold::default());
    m.controller::<Held>();
}

/// A drained call: the service's answer to `path` obtained on a task of its own.
async fn held(app: &o1_support::Running, path: &str) -> (tokio::task::JoinHandle<Response>, Dep<Hold>) {
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let service = app.service();
    let request = Call::get(path).into_request();
    let call = tokio::spawn(async move { service.call(request).await });
    tokio::time::timeout(PATIENCE, hold.entered.notified()).await.expect("the handler is entered in time");
    (call, hold)
}

#[tokio::test]
async fn a_call_whose_response_has_not_begun_when_the_window_ends_is_answered_503_with_connection_close() {
    let app = start_draining(register, WINDOW).await;
    let (call, hold) = held(&app, "/hold").await;
    let closing = app.begin_close();

    let response = tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task");
    let reply = read(response).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.header("connection"), Some("close"));
    assert_eq!(reply.header("retry-after"), Some("1"), "a refusal says when to retry: the server's `shed_retry_after`");
    assert_eq!(reply.header("content-type"), Some("application/problem+json"));
    let document = reply.json();
    assert_eq!((&document["type"], &document["title"], &document["status"]), (&"about:blank".into(), &"Service Unavailable".into(), &503.into()));
    assert_eq!(document["detail"], ulo_transport::SHUTDOWN_MESSAGE, "the one phrase every transport's shutdown refusal carries");
    assert_eq!(reply.matched(), Some(("/hold".to_owned(), "hold")), "the refusal records the route the call had reached");
    assert!(matches!(reply.routing, Some(Routing::Matched { .. })));

    let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task");
    let abandoned = match &shutdown {
        Ok(shutdown) => shutdown.abandoned,
        Err(error) => error.report.abandoned,
    };
    assert_eq!(abandoned, 1, "the held call was abandoned: {shutdown:?}");
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Drain)], "the handler's future dropped, cancelled by the drain");
    app.stop().await;
}

#[tokio::test]
async fn a_call_that_finishes_inside_the_window_is_answered_by_its_handler() {
    let app = start_draining(register, Duration::from_secs(5)).await;
    let (call, hold) = held(&app, "/hold").await;
    let closing = app.begin_close();
    tokio::time::timeout(PATIENCE, app.app().draining()).await.expect("the drain begins");
    hold.release.add_permits(1);

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "released".to_owned()));
    let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task").expect("a clean shutdown");
    assert_eq!(shutdown.abandoned, 0);
    assert_eq!(hold.dropped(), vec![None], "the handler finished, uncancelled");
    app.stop().await;
}

#[tokio::test]
async fn a_response_begun_when_the_window_ends_is_cut_off_with_an_error_frame_not_ended() {
    let app = start_draining(register, WINDOW).await;
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let response = app.call(Call::get("/stream")).await;
    assert_eq!(response.status(), StatusCode::OK, "the head was answered before the drain");
    let (_, mut body) = response.into_parts();
    let first = tokio::time::timeout(PATIENCE, body.frame()).await.expect("first frame in time").expect("a frame").expect("no error yet");
    assert_eq!(first.into_data().ok(), Some(Bytes::from_static(b"first")));
    let reading = tokio::spawn(async move { body.frame().await });
    tokio::time::timeout(PATIENCE, hold.entered.notified()).await.expect("the stream waits for its second chunk");
    let closing = app.begin_close();

    let next = tokio::time::timeout(PATIENCE, reading).await.expect("the read ended in time").expect("the read task");
    let error = next.expect("the body did not end cleanly").expect_err("the body ends with an error frame, not a last chunk");
    let call = error.downcast_ref::<CallError>().expect("the shutdown refusal is the error");
    assert_eq!(call.kind(), ErrorKind::Unavailable);
    assert_eq!(call.message(), ulo_transport::SHUTDOWN_MESSAGE);
    assert_eq!(hold.ends(), vec![StreamOutcome::CutOff(Some(CancelReason::Drain))], "reported cut off by the drain, and once");

    let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task");
    let abandoned = match &shutdown {
        Ok(shutdown) => shutdown.abandoned,
        Err(error) => error.report.abandoned,
    };
    assert_eq!(abandoned, 1, "the begun call was abandoned: {shutdown:?}");
    app.stop().await;
}

#[tokio::test]
async fn a_stream_that_ends_inside_the_window_ends_cleanly() {
    let app = start_draining(register, Duration::from_secs(5)).await;
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let response = app.call(Call::get("/stream")).await;
    let reading = tokio::spawn(read(response));
    tokio::time::timeout(PATIENCE, hold.entered.notified()).await.expect("the stream waits for its second chunk");
    let closing = app.begin_close();
    tokio::time::timeout(PATIENCE, app.app().draining()).await.expect("the drain begins");
    hold.release.add_permits(1);

    let reply = tokio::time::timeout(PATIENCE, reading).await.expect("read in time").expect("the read task");
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "firstsecond".to_owned()));
    let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task").expect("a clean shutdown");
    assert_eq!(shutdown.abandoned, 0);
    assert_eq!(hold.ends(), vec![StreamOutcome::Completed]);
    app.stop().await;
}

#[tokio::test]
async fn a_stream_cancelled_for_another_reason_is_not_cut_off_by_the_drain() {
    let app = start_draining(register, WINDOW).await;
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let response = app.call(Call::get("/cancelled-stream")).await;
    let reading = tokio::spawn(read(response));
    tokio::time::timeout(PATIENCE, hold.entered.notified()).await.expect("the stream waits for its second chunk");
    // The body cancels its execution once the drain begins, and the window ends with it running.
    let shutdown = tokio::time::timeout(PATIENCE, app.begin_close()).await.expect("close finished in time").expect("the close task");
    assert_eq!(shutdown.map_or_else(|error| error.report.abandoned, |shutdown| shutdown.abandoned), 1, "the window ended with the stream open");
    hold.release.add_permits(1);

    let reply = tokio::time::timeout(PATIENCE, reading).await.expect("read in time").expect("the read task");
    assert_eq!(reply.text(), "firstsecond", "the cancellation was the handler's own and the stream ran on");
    assert_eq!(hold.ends(), vec![StreamOutcome::Completed]);
    app.stop().await;
}

#[tokio::test]
async fn a_call_cancelled_for_another_reason_runs_on_and_is_answered_by_its_handler() {
    let app = start_draining(register, WINDOW).await;
    let (call, hold) = held(&app, "/cancelled-hold").await;
    // The handler cancels its execution once the drain begins, and the window ends with it running.
    let shutdown = tokio::time::timeout(PATIENCE, app.begin_close()).await.expect("close finished in time").expect("the close task");
    assert_eq!(shutdown.map_or_else(|error| error.report.abandoned, |shutdown| shutdown.abandoned), 1, "the window ended with the call running");
    hold.release.add_permits(1);

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "released".to_owned()));
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Explicit)], "the handler's own cancellation is the reason it finished under");
    app.stop().await;
}

/// The pipeline is polled before the cancellation: a handler that answers on observing the
/// drain's cancellation is answered as it chose.
#[tokio::test]
async fn a_handler_answering_once_the_drain_cancels_it_is_answered_as_it_chose() {
    let app = start_draining(register, WINDOW).await;
    let (call, hold) = held(&app, "/answers-when-cancelled").await;
    let closing = app.begin_close();

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "answered after the cancellation".to_owned()));
    assert_eq!(reply.header("connection"), None, "the handler's own answer carries no refusal's header");
    assert_eq!(reply.matched(), Some(("/answers-when-cancelled".to_owned(), "answers_when_cancelled")));
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Drain)], "the handler ran on to its end under the drain's cancellation");
    let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task");
    assert_eq!(shutdown.map_or_else(|error| error.report.abandoned, |shutdown| shutdown.abandoned), 1, "the window ended with the call running");
    app.stop().await;
}

/// A stream answered once the drain's end has cancelled the call would be cut before its first
/// frame, so the service checks the drain once more as it returns it and answers the refusal.
#[tokio::test]
async fn a_stream_answered_once_the_drain_cancels_the_call_is_answered_503_instead() {
    let app = start_draining(register, WINDOW).await;
    let (call, hold) = held(&app, "/streams-when-cancelled").await;
    let closing = app.begin_close();

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE, "{}", reply.text());
    assert_eq!(reply.header("connection"), Some("close"));
    assert_eq!(reply.header("retry-after"), Some("1"));
    assert_eq!(reply.header("content-type"), Some("application/problem+json"));
    assert_eq!(reply.json()["detail"], ulo_transport::SHUTDOWN_MESSAGE);
    assert_eq!(reply.matched(), Some(("/streams-when-cancelled".to_owned(), "streams_when_cancelled")), "the refusal keeps the route");
    assert!(hold.ends().is_empty(), "the discarded stream reported an end: {:?}", hold.ends());
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Drain)]);
    let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task");
    assert_eq!(shutdown.map_or_else(|error| error.report.abandoned, |shutdown| shutdown.abandoned), 1);
    app.stop().await;
}

/// The second check covers only a body the backend would write: a 204 carrying a stream owes the
/// client no body, so the drain cuts nothing and the handler's 204 stands.
#[tokio::test]
async fn a_no_content_answer_chosen_once_the_drain_cancels_the_call_is_not_replaced() {
    let app = start_draining(register, WINDOW).await;
    let (call, hold) = held(&app, "/no-content-when-cancelled").await;
    let closing = app.begin_close();

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!(reply.status, StatusCode::NO_CONTENT, "{}", reply.text());
    assert_eq!(reply.header("connection"), None, "the handler's own answer carries no refusal's header");
    assert_eq!(reply.matched(), Some(("/no-content-when-cancelled".to_owned(), "no_content_when_cancelled")));
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Drain)]);
    let _ = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time");
    app.stop().await;
}

/// The second check covers only a body the backend writes: a `HEAD` handler's stream owes the
/// client no body, so the handler's 200 stands.
#[tokio::test]
async fn a_head_answered_by_a_stream_once_the_drain_cancels_the_call_is_not_replaced() {
    let app = start_draining(register, WINDOW).await;
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let service = app.service();
    let request = Call::new(http::Method::HEAD, "/head-streams-when-cancelled").into_request();
    let call = tokio::spawn(async move { service.call(request).await });
    tokio::time::timeout(PATIENCE, hold.entered.notified()).await.expect("the handler is entered in time");
    let closing = app.begin_close();

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!(reply.status, StatusCode::OK, "{}", reply.text());
    assert_eq!(reply.header("connection"), None, "the handler's own answer carries no refusal's header");
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Drain)]);
    let _ = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time");
    app.stop().await;
}

/// The second check reads the drain's cancellation alone: a stream answered by a call its own
/// handler cancelled is written as it chose.
#[tokio::test]
async fn a_stream_answered_by_a_call_cancelled_for_another_reason_is_not_replaced() {
    let app = start_draining(register, WINDOW).await;
    let (call, hold) = held(&app, "/cancelled-then-streams").await;
    let shutdown = tokio::time::timeout(PATIENCE, app.begin_close()).await.expect("close finished in time").expect("the close task");
    assert_eq!(shutdown.map_or_else(|error| error.report.abandoned, |shutdown| shutdown.abandoned), 1, "the window ended with the call running");
    hold.release.add_permits(1);

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "released".to_owned()));
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Explicit)]);
    app.stop().await;
}

/// Reads `body` to its end: its data frames as text, and the error that ended it.
async fn frames_to_the_error(body: &mut HttpBody) -> (Vec<String>, BoxError) {
    let mut data = Vec::new();
    loop {
        match tokio::time::timeout(PATIENCE, body.frame()).await.expect("a frame in time") {
            Some(Ok(frame)) => data.push(String::from_utf8_lossy(&frame.into_data().expect("a data frame")).into_owned()),
            Some(Err(error)) => return (data, error),
            None => panic!("the body ended cleanly after {data:?}"),
        }
    }
}

fn shutting_down(error: &BoxError) {
    let call = error.downcast_ref::<CallError>().unwrap_or_else(|| panic!("the error frame is not the shutdown refusal: {error}"));
    assert_eq!((call.kind(), call.message()), (ErrorKind::Unavailable, ulo_transport::SHUTDOWN_MESSAGE));
}

#[tokio::test]
async fn a_body_whose_frames_are_always_ready_takes_no_frame_past_the_poll_its_cancellation_is_observed_in() {
    let app = start_draining(register, Duration::from_secs(5)).await;
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let response = app.call(Call::get("/self-drained")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let (_, mut body) = response.into_parts();

    let (data, error) = frames_to_the_error(&mut body).await;
    assert_eq!(data, ["1", "2", "3"], "the frames written before the error frame");
    shutting_down(&error);
    assert_eq!(hold.produced.load(Ordering::SeqCst), CANCELLED_AT, "a frame was taken after the cancellation");
    assert_eq!(hold.ends(), vec![StreamOutcome::CutOff(Some(CancelReason::Drain))], "reported cut off by the drain, and once");
    drop(body);
    app.stop().await;
}

#[tokio::test(flavor = "current_thread")]
async fn a_frame_ready_in_the_poll_the_cancellation_wakes_is_written_and_none_after_it() {
    let app = start_draining(register, Duration::from_secs(5)).await;
    let hold = app.app().get::<Hold>().await.expect("the app binds its hold");
    let response = app.call(Call::get("/gated")).await;
    let (_, mut body) = response.into_parts();
    let first = tokio::time::timeout(PATIENCE, body.frame()).await.expect("first frame in time").expect("a frame").expect("no error yet");
    assert_eq!(first.into_data().ok(), Some(Bytes::from_static(b"1")));
    {
        let mut next = std::pin::pin!(body.frame());
        tokio::select! {
            biased;
            frame = &mut next => panic!("the second frame came before its gate opened: {:?}", frame.map(|frame| frame.map(|_| ()))),
            () = hold.waiting.notified() => {}
        }
        // Both are ready before the body's next poll: the frame is polled first.
        let exec = hold.exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
        exec.cancel_with(CancelReason::Drain);
        hold.gate.notify_one();
        let second = tokio::time::timeout(PATIENCE, next).await.expect("the second frame in time").expect("a frame");
        let second = second.unwrap_or_else(|error| panic!("the frame ready in the cancellation's poll was replaced by {error}"));
        assert_eq!(second.into_data().ok(), Some(Bytes::from_static(b"2")));
    }

    let (data, error) = frames_to_the_error(&mut body).await;
    assert!(data.is_empty(), "frames taken after the poll the cancellation woke: {data:?}");
    shutting_down(&error);
    assert_eq!(hold.produced.load(Ordering::SeqCst), 2, "a frame was produced after the poll the cancellation woke");
    assert_eq!(hold.ends(), vec![StreamOutcome::CutOff(Some(CancelReason::Drain))], "reported cut off by the drain, and once");
    drop(body);
    app.stop().await;
}
