//! A call still running when the drain window ends is abandoned: its execution is cancelled with
//! `CancelReason::Drain` and the call is answered with the one shutdown refusal. A response not yet
//! begun is 503 with `Connection: close` and the problem document the other refusals use; one
//! begun is a body that errors, which a backend writes as a closed connection (HTTP/1.1) or a reset
//! stream (HTTP/2), never as the terminating chunk or `END_STREAM`. A call that finishes inside the
//! window is untouched.

mod o1_support;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream;
use http_body_util::BodyExt;
use o1_support::{Call, PATIENCE, read, start_draining};
use tokio::sync::{Notify, Semaphore};
use ulo::{CancelReason, Dep, ExecutionRef, ModuleDef, StreamOutcome, injectable, routes};
use ulo_http::{Bytes, HttpBody, Response, Routing, StatusCode};
use ulo_transport::{CallError, ErrorKind};

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
}

impl Default for Hold {
    fn default() -> Self {
        Hold {
            release: Semaphore::new(0),
            entered: Notify::new(),
            dropped: Mutex::new(Vec::new()),
            ends: Mutex::new(Vec::new()),
            entries: AtomicUsize::new(0),
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

    /// As `/hold`, its execution already cancelled by its own handler.
    #[ulo_http::get("/cancelled-hold")]
    async fn cancelled_hold(&self, exec: ExecutionRef) -> &'static str {
        exec.cancel();
        self.hold(exec).await
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

    /// As `/stream`, its execution already cancelled by its own handler: a reason that is not the
    /// drain's.
    #[ulo_http::get("/cancelled-stream")]
    async fn cancelled_stream(&self, exec: ExecutionRef) -> Response {
        exec.cancel();
        self.stream(exec).await
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
    assert!(reply.header("retry-after").is_some(), "a refusal says when to retry");
    assert_eq!(reply.header("content-type"), Some("application/problem+json"));
    let document = reply.json();
    assert_eq!(document["status"], 503);
    assert_eq!(document["detail"], "the server is shutting down", "the one phrase every transport's shutdown refusal carries");
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
    assert_eq!(call.message(), "the server is shutting down");
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
    hold.release.add_permits(1);

    let reply = read(tokio::time::timeout(PATIENCE, call).await.expect("answered in time").expect("the call task")).await;
    assert_eq!((reply.status, reply.text()), (StatusCode::OK, "released".to_owned()));
    assert_eq!(hold.dropped(), vec![Some(CancelReason::Explicit)], "the handler's own cancellation is the reason it finished under");
    app.stop().await;
}
