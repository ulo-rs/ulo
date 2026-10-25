//! What a client reads on the wire when the drain window ends with its call still running, through
//! the hyper backend: a response not yet begun is the 503 refusal with `Connection: close`, the
//! connection then closed; one begun ends with the connection closed in the middle of the chunked
//! body, the terminating chunk never written, so the client can tell a cut-off stream from a
//! finished one. A call that finishes inside the window is answered whole. A stream answered at
//! about the window's end reads as one of those two or, where the drain ends during the hand-off
//! to hyper, as a connection closed with nothing written; never as a 503 after a 200.
//!
//! The client is raw HTTP/1.1 over a socket, reading until the server closes the connection.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, Semaphore};
use ulo::{App, AppHandle, Dep, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_http::{Bytes, HttpBody, Response};

/// The longest any one wait below lasts before it fails the test.
const PATIENCE: Duration = Duration::from_secs(10);

/// The drain window the held call outlives.
const WINDOW: Duration = Duration::from_millis(400);

/// What the held handlers share with the test.
struct Hold {
    release: Semaphore,
    entered: Notify,
    /// How long `/late` waits before it answers.
    late: Duration,
}

#[derive(Clone)]
struct Shared(Arc<Hold>);

#[injectable]
struct Held {
    hold: Dep<Shared>,
}

#[routes]
impl Held {
    #[ulo_http::get("/hold")]
    async fn hold(&self) -> &'static str {
        self.hold.0.entered.notify_one();
        self.hold.0.release.acquire().await.expect("never closed").forget();
        "released"
    }

    /// After `late`, a stream that never ends: `first`, then nothing.
    #[ulo_http::get("/late")]
    async fn late(&self) -> Response {
        self.hold.0.entered.notify_one();
        tokio::time::sleep(self.hold.0.late).await;
        let body = stream::iter([Ok::<_, Infallible>(Bytes::from_static(b"first"))]).chain(stream::pending());
        Response::new(HttpBody::stream(body))
    }

    /// `first` at once, then `second` once released.
    #[ulo_http::get("/stream")]
    async fn stream(&self) -> Response {
        let hold = Arc::clone(&self.hold.0);
        let body = stream::unfold(0, move |step| {
            let hold = Arc::clone(&hold);
            async move {
                match step {
                    0 => Some((Ok::<_, Infallible>(Bytes::from_static(b"first")), 1)),
                    1 => {
                        hold.entered.notify_one();
                        hold.release.acquire().await.expect("never closed").forget();
                        Some((Ok(Bytes::from_static(b"second")), 2))
                    }
                    _ => None,
                }
            }
        });
        Response::new(HttpBody::stream(body))
    }
}

struct Root(Shared);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Held>();
    }
}

struct Served {
    hold: Shared,
    handle: AppHandle,
    addr: SocketAddr,
    serving: tokio::task::JoinHandle<()>,
}

async fn serve(window: Duration) -> Served {
    serve_late(window, Duration::ZERO).await
}

async fn serve_late(window: Duration, late: Duration) -> Served {
    let hold = Shared(Arc::new(Hold { release: Semaphore::new(0), entered: Notify::new(), late }));
    let app = App::builder(Root(hold.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .drain_timeout(window)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the app listens");
    let addr = app.addresses().first().expect("one address").addr;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Served { hold, handle, addr, serving }
}

impl Served {
    /// Sends `GET path` and waits until the handler holds it.
    async fn start(&self, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect(self.addr).await.expect("connects");
        stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: drain\r\n\r\n").as_bytes()).await.expect("writes the request");
        tokio::time::timeout(PATIENCE, self.hold.0.entered.notified()).await.expect("the handler is entered in time");
        stream
    }

    fn close(&self) -> tokio::task::JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>> {
        let handle = self.handle.clone();
        tokio::spawn(async move { handle.close(Signal::new("test")).await })
    }

    async fn finish(self, closing: tokio::task::JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>>) -> usize {
        let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time").expect("the close task");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
        match shutdown {
            Ok(shutdown) => shutdown.abandoned,
            Err(error) => error.report.abandoned,
        }
    }
}

/// Everything the server writes until it closes the connection.
async fn read_to_close(stream: &mut TcpStream) -> Vec<u8> {
    let mut seen = Vec::new();
    tokio::time::timeout(PATIENCE, stream.read_to_end(&mut seen)).await.expect("the server closed the connection in time").ok();
    seen
}

fn head_of(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).split("\r\n\r\n").next().unwrap_or_default().to_ascii_lowercase()
}

fn status_of(raw: &[u8]) -> Option<u16> {
    String::from_utf8_lossy(raw).lines().next()?.split_whitespace().nth(1)?.parse().ok()
}

fn body_of(raw: &[u8]) -> String {
    String::from_utf8_lossy(raw).split_once("\r\n\r\n").map(|(_, body)| body.to_owned()).unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_abandoned_before_its_response_began_reads_503_with_connection_close_then_the_connection_closes() {
    let served = serve(WINDOW).await;
    let mut stream = served.start("/hold").await;
    let closing = served.close();

    let raw = read_to_close(&mut stream).await;
    assert_eq!(status_of(&raw), Some(503), "{:?}", String::from_utf8_lossy(&raw));
    assert!(head_of(&raw).lines().any(|line| line == "connection: close"), "{}", head_of(&raw));
    assert!(head_of(&raw).lines().any(|line| line == "content-type: application/problem+json"), "{}", head_of(&raw));
    assert!(head_of(&raw).lines().any(|line| line == "retry-after: 1"), "the server's `shed_retry_after`: {}", head_of(&raw));
    let document: serde_json::Value = serde_json::from_str(&body_of(&raw)).expect("a problem document");
    assert_eq!((&document["type"], &document["title"], &document["status"]), (&"about:blank".into(), &"Service Unavailable".into(), &503.into()));
    assert_eq!(document["detail"], "the server is shutting down");
    assert_eq!(served.finish(closing).await, 1, "the held call was abandoned");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_response_abandoned_after_it_began_is_cut_in_the_middle_of_its_chunked_body() {
    let served = serve(WINDOW).await;
    let mut stream = served.start("/stream").await;
    let closing = served.close();

    let raw = read_to_close(&mut stream).await;
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert_eq!(status_of(&raw), Some(200), "{text:?}");
    assert!(head_of(&raw).lines().any(|line| line == "transfer-encoding: chunked"), "{}", head_of(&raw));
    assert!(body_of(&raw).contains("5\r\nfirst\r\n"), "the chunk written before the cut: {text:?}");
    assert!(!body_of(&raw).contains("second"), "{text:?}");
    assert!(!raw.ends_with(b"0\r\n\r\n"), "the terminating chunk marks a body that finished, and this one did not: {text:?}");
    assert_eq!(served.finish(closing).await, 1, "the begun call was abandoned");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_response_that_finishes_inside_the_window_ends_with_the_terminating_chunk() {
    let served = serve(Duration::from_secs(5)).await;
    let mut stream = served.start("/stream").await;
    let closing = served.close();
    tokio::time::timeout(PATIENCE, served.handle.draining()).await.expect("the drain begins");
    served.hold.0.release.add_permits(1);

    let raw = read_to_close(&mut stream).await;
    let text = String::from_utf8_lossy(&raw).into_owned();
    assert_eq!(status_of(&raw), Some(200), "{text:?}");
    assert!(body_of(&raw).contains("6\r\nsecond\r\n"), "{text:?}");
    assert!(raw.ends_with(b"0\r\n\r\n"), "a finished body ends with the terminating chunk: {text:?}");
    assert_eq!(served.finish(closing).await, 0);
}

/// How a call answered at about the drain window's end read on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Read {
    /// The 503 refusal, whole.
    Refused,
    /// A 200 whose chunked body ended with no terminating chunk.
    CutOff,
    /// The connection closed with nothing written.
    Nothing,
}

fn classify(raw: &[u8]) -> Read {
    let text = String::from_utf8_lossy(raw).into_owned();
    if raw.is_empty() {
        return Read::Nothing;
    }
    assert_eq!(text.matches("HTTP/1.1 ").count(), 1, "one response and no second one after it: {text:?}");
    match status_of(raw) {
        Some(503) => {
            assert!(head_of(raw).lines().any(|line| line == "connection: close"), "{text:?}");
            let document: serde_json::Value = serde_json::from_str(&body_of(raw)).expect("a problem document, whole");
            assert_eq!(document["detail"], "the server is shutting down", "{text:?}");
            Read::Refused
        }
        Some(200) => {
            assert!(head_of(raw).lines().any(|line| line == "transfer-encoding: chunked"), "{text:?}");
            assert!(!raw.ends_with(b"0\r\n\r\n"), "an endless stream cannot have finished: {text:?}");
            Read::CutOff
        }
        other => panic!("a status other than 503 or 200: {other:?} in {text:?}"),
    }
}

/// An endless stream answered within 8 ms either side of the drain window's end, 200 times, each
/// on an app of its own. The service checks the drain once more as it returns a stream, so a
/// drain that has ended by then is answered 503; what is left is the hand-off to hyper, where a
/// drain ending before the head is flushed closes the connection with nothing written. The counts
/// are printed: `--nocapture` shows how often each reading occurred.
///
/// It stays green with that second check removed, since more runs then read nothing, which is one
/// of the three readings it accepts. `ulo-http`'s `tests/drain.rs ::
/// a_stream_answered_once_the_drain_cancels_the_call_is_answered_503_instead` is the test that
/// decides the second check; this one guards against a 503 written after a 200.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stream_answered_at_the_windows_end_reads_as_a_503_a_cut_off_200_or_nothing_never_both() {
    const WINDOW_MS: u64 = 30;
    const RUNS: u64 = 200;
    const BATCH: u64 = 8;
    let mut counts = std::collections::HashMap::<Read, usize>::new();
    for batch in 0..RUNS / BATCH {
        let runs = (0..BATCH).map(|i| {
            // -8 ms to +8 ms around the window's end, spread over the runs.
            let offset = ((batch * BATCH + i) % 17) as i64 - 8;
            let late = Duration::from_millis((WINDOW_MS as i64 + offset) as u64);
            tokio::spawn(async move {
                let served = serve_late(Duration::from_millis(WINDOW_MS), late).await;
                let mut stream = served.start("/late").await;
                let closing = served.close();
                let raw = read_to_close(&mut stream).await;
                let _ = served.finish(closing).await;
                classify(&raw)
            })
        });
        for run in futures_util::future::join_all(runs).await {
            *counts.entry(run.expect("the run task")).or_default() += 1;
        }
    }
    eprintln!("readings over {RUNS} runs: {counts:?}");
    assert_eq!(counts.values().sum::<usize>(), RUNS as usize);
}
