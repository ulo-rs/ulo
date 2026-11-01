//! A request's execution ends when the service drops it with the response body (DESIGN §3.8): an
//! execution-scoped service holding its own `ExecutionRef`, DESIGN §5's `AuditContext`, drops as
//! the reply finishes, so the drain has nothing to wait for; and a streaming body keeps its
//! execution, its instances and its lookups, until the stream ends, the drain waiting for it.
//!
//! The app's clock is a `ScriptedTimer` that nothing advances, so a close that waited out the
//! drain window would never return.

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Notify;
use ulo::{App, AppHandle, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Shutdown, ShutdownError, Signal, injectable, routes};
use ulo_conformance_support::ScriptedTimer;
use ulo_http::{Bytes, HttpBody, Response};

const PATIENCE: Duration = Duration::from_secs(10);
const DRAIN: Duration = Duration::from_secs(10);

/// What the handlers share with the test.
#[derive(Default)]
struct Probe {
    dropped: AtomicUsize,
    /// The streaming call's execution, for the test to cancel.
    exec: Mutex<Option<ExecutionRef>>,
    streaming: Notify,
}

#[derive(Clone, Default)]
struct Shared(Arc<Probe>);

#[injectable(execution)]
struct Audit {
    exec: ExecutionRef,
    probe: Dep<Shared>,
}

impl Drop for Audit {
    fn drop(&mut self) {
        self.probe.0.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

#[injectable]
struct Audited {
    audit: Dep<Audit>,
}

#[routes]
impl Audited {
    #[ulo_http::get("/audit")]
    fn audit(&self) -> &'static str {
        if self.audit.exec.is_cancelled() { "cancelled" } else { "ok" }
    }

    /// `first`, then, once the execution is cancelled, whether a lookup through the instance's own
    /// handle still answers that instance: `same`, or `ended` when the execution ended first.
    #[ulo_http::get("/stream")]
    fn stream(&self) -> Response {
        let audit = self.audit.clone();
        *audit.probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner) = Some(audit.exec.clone());
        let body = stream::unfold(0, move |step| {
            let audit = audit.clone();
            async move {
                match step {
                    0 => Some((Ok::<_, Infallible>(Bytes::from_static(b"first\n")), 1)),
                    1 => {
                        audit.probe.0.streaming.notify_one();
                        audit.exec.cancelled().await;
                        let text: &'static [u8] = match audit.exec.get::<Audit>().await {
                            Ok(again) if Arc::ptr_eq(again.arc(), audit.arc()) => b"same",
                            Ok(_) => b"another",
                            Err(_) => b"ended",
                        };
                        Some((Ok(Bytes::from_static(text)), 2))
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
        m.provide::<Audit>();
        m.controller::<Audited>();
    }
}

struct Served {
    probe: Shared,
    clock: ScriptedTimer,
    handle: AppHandle,
    addr: SocketAddr,
    serving: tokio::task::JoinHandle<()>,
}

async fn serve() -> Served {
    let probe = Shared::default();
    let clock = ScriptedTimer::new();
    let app = App::builder(Root(probe.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .drain_timeout(DRAIN)
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
    Served { probe, clock, handle, addr, serving }
}

impl Served {
    async fn request(&self, path: &str) -> TcpStream {
        let mut stream = TcpStream::connect(self.addr).await.expect("connects");
        let request = format!("GET {path} HTTP/1.1\r\nHost: end\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.expect("writes the request");
        stream
    }

    fn dropped(&self) -> usize {
        self.probe.0.dropped.load(Ordering::SeqCst)
    }

    fn close(&self) -> tokio::task::JoinHandle<Result<Shutdown, ShutdownError>> {
        let handle = self.handle.clone();
        tokio::spawn(async move { handle.close(Signal::new("test")).await })
    }

    async fn finish(self, closing: tokio::task::JoinHandle<Result<Shutdown, ShutdownError>>) -> Shutdown {
        let shutdown = tokio::time::timeout(PATIENCE, closing).await.expect("close returned without the clock passing the drain window");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
        assert_eq!(self.clock.elapsed(), Duration::ZERO, "nothing advanced the clock");
        shutdown.expect("the close task").expect("a clean shutdown")
    }
}

async fn read_to_close(stream: &mut TcpStream) -> String {
    let mut seen = Vec::new();
    tokio::time::timeout(PATIENCE, stream.read_to_end(&mut seen)).await.expect("the server closed the connection in time").ok();
    String::from_utf8_lossy(&seen).into_owned()
}

/// Waits for `ready` within [`PATIENCE`].
async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    let waited = async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(PATIENCE, waited).await.unwrap_or_else(|_| panic!("{what} within {PATIENCE:?}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn three_finished_calls_whose_instance_holds_its_execution_leave_the_drain_nothing_to_wait_for() {
    let served = serve().await;
    for n in 1..=3 {
        let mut stream = served.request("/audit").await;
        let reply = read_to_close(&mut stream).await;
        assert!(reply.starts_with("HTTP/1.1 200") && reply.ends_with("ok"), "{reply:?}");
        until("the call's `Audit` dropping as its reply finished", || served.dropped() == n).await;
    }
    let closing = served.close();
    let shutdown = served.finish(closing).await;
    assert_eq!((shutdown.abandoned, shutdown.terminal_skipped), (0, 0));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_streaming_body_keeps_its_execution_until_it_ends_and_the_drain_waits_for_it() {
    let served = serve().await;
    let mut stream = served.request("/stream").await;
    tokio::time::timeout(PATIENCE, served.probe.0.streaming.notified()).await.expect("the body reached its wait");
    assert_eq!(served.dropped(), 0, "the instance lives while the body runs");

    let closing = served.close();
    until("the drain waiting on its window", || served.clock.live(DRAIN) > 0).await;
    assert!(!closing.is_finished(), "the drain waits for the open stream's execution");

    let exec = served.probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
    exec.cancel();
    let reply = read_to_close(&mut stream).await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply:?}");
    assert!(reply.contains("first\n") && reply.contains("same"), "the body's lookup after `cancelled()` answered its own instance: {reply:?}");
    assert!(reply.ends_with("0\r\n\r\n"), "the stream ended with its terminating chunk: {reply:?}");

    let probe = served.probe.clone();
    let shutdown = served.finish(closing).await;
    assert_eq!(shutdown.abandoned, 0, "the stream's execution ended with the stream, inside the window");
    assert_eq!(probe.0.dropped.load(Ordering::SeqCst), 1, "the instance dropped with the body");
    assert!(matches!(exec.get::<Audit>().await, Err(ulo::LookupError::ExecutionEnded { .. })), "the call's handle outlived it");
}

/// Reads one response whose body is `body` from a kept-alive connection.
async fn read_reply(stream: &mut TcpStream, body: &str) -> String {
    let mut seen = Vec::new();
    let read = async {
        let mut buf = [0u8; 1024];
        loop {
            let n = stream.read(&mut buf).await.expect("reads");
            assert!(n > 0, "the server closed the kept-alive connection: {:?}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&seen);
            if text.contains("\r\n\r\n") && text.ends_with(body) {
                return text.into_owned();
            }
        }
    };
    tokio::time::timeout(PATIENCE, read).await.expect("the reply arrived in time")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_on_a_kept_alive_connection_end_their_executions_while_the_connection_stays_open() {
    let served = serve().await;
    let mut stream = TcpStream::connect(served.addr).await.expect("connects");
    for n in 1..=3 {
        stream.write_all(b"GET /audit HTTP/1.1\r\nHost: end\r\n\r\n").await.expect("writes the request");
        let reply = read_reply(&mut stream, "ok").await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply:?}");
        until("the call's `Audit` dropping while its connection stays open", || served.dropped() == n).await;
    }
    for n in 4..=6 {
        stream.write_all(b"HEAD /audit HTTP/1.1\r\nHost: end\r\n\r\n").await.expect("writes the request");
        let reply = read_reply(&mut stream, "\r\n\r\n").await;
        assert!(reply.starts_with("HTTP/1.1 200"), "{reply:?}");
        until("a HEAD call's `Audit` dropping with its unwritten body", || served.dropped() == n).await;
    }
    let closing = served.close();
    let shutdown = served.finish(closing).await;
    assert_eq!(shutdown.abandoned, 0);
    drop(stream);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_peer_leaving_mid_stream_ends_the_streams_execution() {
    let served = serve().await;
    let stream = served.request("/stream").await;
    tokio::time::timeout(PATIENCE, served.probe.0.streaming.notified()).await.expect("the body reached its wait");
    drop(stream);
    until("the stream's `Audit` dropping once the peer left", || served.dropped() == 1).await;
    let exec = served.probe.0.exec.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the handler kept its execution");
    assert_eq!(exec.cancel_reason(), Some(ulo::CancelReason::Disconnected));
    assert!(matches!(exec.get::<Audit>().await, Err(ulo::LookupError::ExecutionEnded { .. })));
    let closing = served.close();
    let shutdown = served.finish(closing).await;
    assert_eq!(shutdown.abandoned, 0);
}
