//! `CUT_GRACE` runs from the end of a `close`, completed or cut (transports DESIGN §5.2, "A cut
//! `close`"). A server whose `close` the app's spent `shutdown_timeout` cuts after one poll still
//! holds each connection for the grace, and drops it at the grace. A client whose `close` waits
//! for its writer, held by a server that does not read, holds its connection for the grace from
//! the moment that wait ends, not from the moment `close` began.
//!
//! The clock both links read is scripted: no sleep it hands out ends until the test advances it,
//! and the test reads which sleeps are live and the clock time each was last polled at. Each test
//! sizes what it queues from the buffers the kernel granted the peer's socket, read back, so the
//! buffers cannot hold it whatever the platform's defaults.

use std::error::Error;
use std::fmt;
use std::future::pending;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ulo::{App, BoxFuture, FailureReason, Module, ModuleDef, ModuleIdentity, Runtime, ShutdownFailure, Signal, Spawn, TaskHandle, Timer, injectable, routes};
use ulo_conformance_support::ScriptedTimer;
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern, Payload, ReplyTo};
use ulo_rpc_tcp::Tcp;
use ulo_rpc_tcp_core::CUT_GRACE;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, ErrorKind as Kind};

const ANSWER: &str = "cut_grace.answer";

/// The size of each answer or request frame a test queues.
const FRAME_BYTES: usize = 64 * 1024;

/// How many times over the peer's granted buffers a test queues frames or has the peer write:
/// more than the two sockets' buffers hold.
const OVER_BUFFERS: usize = 16;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long the client's `close` takes on the scripted clock, past `CUT_GRACE`: its writer is held
/// until then, so a close that does not wait for its writer, or a grace counted from the close's
/// start, drops the writer mid-request.
const CLOSE_TAKES: Duration = Duration::from_secs(3);

/// The bytes the kernel granted the peer's send and receive buffers, read back: the stand-in for
/// the link's socket at the other end, which the test cannot reach.
fn buffered(peer: &TcpStream) -> u64 {
    let socket = socket2::SockRef::from(peer);
    let granted = socket.send_buffer_size().expect("the peer's SO_SNDBUF") + socket.recv_buffer_size().expect("the peer's SO_RCVBUF");
    granted as u64
}

/// How many frames of [`FRAME_BYTES`] a test queues on `peer`'s connection.
fn frames(peer: &TcpStream) -> u64 {
    (OVER_BUFFERS as u64 * buffered(peer)).div_ceil(FRAME_BYTES as u64)
}

/// A scripted clock with a runtime's spawning: what a client's app hands its link's `connect`.
struct Handed<S> {
    clock: ScriptedTimer,
    spawn: S,
}

impl<S: Spawn> Timer for Handed<S> {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        Timer::try_sleep(&self.clock, d)
    }
}

impl<S: Spawn> Spawn for Handed<S> {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawn.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        Spawn::try_spawn(&self.spawn, fut)
    }
}

/// Waits until `ready` holds, failing the test with `what` after `PATIENCE`.
async fn gate(what: &str, mut ready: impl FnMut() -> bool) {
    let began = Instant::now();
    while !ready() {
        assert!(began.elapsed() < PATIENCE, "{what}, not within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

async fn received<T>(what: &str, receiver: &mpsc::Receiver<T>) -> T {
    let mut value = None;
    gate(what, || match receiver.try_recv() {
        Ok(received) => {
            value = Some(received);
            true
        }
        Err(_) => false,
    })
    .await;
    value.expect("the gate opened on a value")
}

fn write_frame(stream: &mut TcpStream, frame: &Frame) -> io::Result<()> {
    let bytes = Codec::Json.encode_frame(frame).expect("the frame encodes");
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)
}

/// Reads frames from a thread of its own until the connection ends, answering how many frames of
/// `kind` it read and how the read ended: `Ok` at a clean end between frames, the link's FIN.
/// `cancel` frames are passed over.
fn read_all(stream: &TcpStream, kind: &'static str) -> mpsc::Receiver<(u64, io::Result<()>)> {
    let mut stream = stream.try_clone().expect("the peer's socket clones");
    // macOS refuses the option with `EINVAL` once the connection has closed both ways, where no
    // read can block: what is buffered is read, then the end.
    let timeout = match stream.set_read_timeout(Some(PATIENCE / 2)) {
        Err(error) if error.kind() == ErrorKind::InvalidInput => Ok(()),
        other => other,
    };
    let (done, read) = mpsc::channel();
    thread::spawn(move || {
        let mut count = 0;
        let end = timeout.and_then(|()| loop {
            let mut prefix = [0u8; 4];
            match stream.read(&mut prefix[..1]) {
                Ok(0) => break Ok(()),
                Ok(_) => {}
                Err(error) => break Err(error),
            }
            if let Err(error) = stream.read_exact(&mut prefix[1..]) {
                break Err(error);
            }
            let mut body = vec![0u8; u32::from_be_bytes(prefix) as usize];
            if let Err(error) = stream.read_exact(&mut body) {
                break Err(error);
            }
            match (kind, Codec::Json.decode_frame(&body)) {
                ("res", Ok(Frame::Res { .. })) | ("req", Ok(Frame::Req { .. })) => count += 1,
                (_, Ok(Frame::Cancel { .. })) => {}
                (_, other) => break Err(io::Error::other(format!("the link wrote a frame other than `{kind}` or `cancel`: {other:?}"))),
            }
        });
        let _ = done.send((count, end));
    });
    read
}

/// Writes `cancel` frames naming no call from a thread of its own until a write fails; `written`
/// counts the bytes written, and the thread answers the failure.
struct Writing {
    written: Arc<AtomicU64>,
    ended: mpsc::Receiver<io::Error>,
}

impl Writing {
    fn start(stream: &TcpStream) -> Writing {
        let mut stream = stream.try_clone().expect("the peer's socket clones");
        let mut batch = Vec::new();
        for _ in 0..512 {
            let bytes = Codec::Json.encode_frame(&Frame::Cancel { id: u64::MAX }).expect("the cancel encodes");
            batch.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            batch.extend_from_slice(&bytes);
        }
        let written = Arc::new(AtomicU64::new(0));
        let (end, ended) = mpsc::channel();
        let counting = Arc::clone(&written);
        thread::spawn(move || {
            loop {
                if let Err(error) = stream.write_all(&batch) {
                    let _ = end.send(error);
                    return;
                }
                counting.fetch_add(batch.len() as u64, Ordering::Release);
            }
        });
        Writing { written, ended }
    }

    fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    fn failed(&self) -> Option<io::Error> {
        self.ended.try_recv().ok()
    }

    /// Waits until the link has read more than `over` bytes past what was written so far, or a
    /// write fails, and answers the failure.
    async fn read_through(&self, what: &str, over: u64) -> Option<io::Error> {
        let before = self.written();
        let mut failed = None;
        gate(what, || {
            failed = self.failed();
            failed.is_some() || self.written() > before + over
        })
        .await;
        failed
    }

    async fn fails(&self, what: &str) -> io::Error {
        let mut failed = None;
        gate(what, || {
            failed = self.failed();
            failed.is_some()
        })
        .await;
        failed.expect("the gate opened on a failure")
    }
}

/// Whether `error` is what a write meets on a connection its peer dropped with bytes unread.
fn reset(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe)
}

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> Kind {
        Kind::Internal
    }
}

/// Calls the server has answered.
static ANSWERED: AtomicU64 = AtomicU64::new(0);

#[injectable]
struct Answering;

#[routes]
impl Answering {
    #[ulo_rpc::message("cut_grace.answer")]
    async fn answer(&self, n: Payload<u32>) -> Result<String, Never> {
        let _ = n;
        ANSWERED.fetch_add(1, Ordering::SeqCst);
        Ok("a".repeat(FRAME_BYTES))
    }
}

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Answering>();
    }
}

/// The app's spent cap cuts the server's `close` after one poll. The connection, its peer half
/// closed and reading none of the answers queued for it, is held until `CUT_GRACE` after the cut
/// and dropped there, its writer with answers still queued.
///
/// On one thread, so the connection's task cannot be aborted and joined while that one poll runs,
/// which would complete the `close` instead of cutting it.
#[tokio::test]
async fn a_cut_close_holds_a_half_closed_peer_s_connection_for_the_grace() {
    let clock = ScriptedTimer::new();
    let app = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .timer(clock.clone())
        .shutdown_timeout(Duration::ZERO)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(pending::<Signal>()).await });

    let mut peer = TcpStream::connect(addr).expect("the server accepts a connection");
    let calls = frames(&peer);
    for id in 1..=calls {
        let request = Frame::Req { id, pattern: ANSWER.to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
        write_frame(&mut peer, &request).expect("the request is written");
    }
    gate("the server answers every call", || ANSWERED.load(Ordering::SeqCst) == calls).await;

    let closed = handle.close(Signal::new("cut_grace")).await;
    let cut = closed.as_ref().err().is_some_and(|error| {
        error.failures.iter().any(|failure| matches!(failure, ShutdownFailure::Close { reason: FailureReason::TimedOut { .. }, .. }))
    });
    assert!(cut, "the spent cap did not cut the server's close: {closed:?}");
    peer.shutdown(Shutdown::Write).expect("the peer shuts its half");
    gate("the connection asks the app's clock for its grace", || clock.live(CUT_GRACE) == 1).await;

    clock.advance(CUT_GRACE - Duration::from_millis(1));
    gate("the connection's grace sees the clock short of it", || clock.pending_at(CUT_GRACE) == Some(clock.elapsed())).await;
    assert_eq!(clock.live(CUT_GRACE), 1, "the connection was dropped short of the grace");

    clock.advance(Duration::from_millis(1));
    gate("the grace drops the connection", || clock.live(CUT_GRACE) == 0).await;
    let (answers, end) = received("the peer's read ends", &read_all(&peer, "res")).await;
    assert!(
        end.as_ref().map_or_else(|error| error.kind() == ErrorKind::UnexpectedEof, |()| true),
        "the peer's read after {answers} answers ended {end:?}, not with the server's FIN"
    );
    assert!(answers < calls, "the peer read all {calls} answers: the writer outlived the grace");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("the app's serve ends at its close");
}

/// A server that accepts one connection on a thread of its own and hands it over unread.
fn silent_peer() -> (SocketAddr, mpsc::Receiver<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the peer's address");
    let (accepted, connection) = mpsc::channel();
    thread::spawn(move || {
        let (stream, _) = listener.accept().expect("the client connects");
        let _ = accepted.send(stream);
    });
    (addr, connection)
}

/// The client's `close` waits for its writer, held by a server reading none of the requests queued
/// before the close, until the server reads them [`CLOSE_TAKES`] into it, past `CUT_GRACE`. The
/// server reads every request and the client's FIN, and, still writing, is read until `CUT_GRACE`
/// after the close's end and reset there. A close that did not wait for its writer would drop it
/// at `CUT_GRACE` after the close's start, the server reading a reset after some of the requests.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_s_grace_runs_from_the_end_of_a_close_that_took_time() {
    let clock = ScriptedTimer::new();
    let (addr, accepted) = silent_peer();
    let handed: Arc<dyn Runtime> = Arc::new(Handed { clock: clock.clone(), spawn: Tokio::current() });
    let client = Arc::new(Tcp::new(addr));
    let outbound = client.connect(&handed).await.expect("the client connects");
    let peer = received("the client's connection reaches the server", &accepted).await;

    let requests = frames(&peer);
    let data = Data::new(format!("\"{}\"", "a".repeat(FRAME_BYTES)).into_bytes());
    for id in 1..=requests {
        let request = Frame::Req { id, pattern: ANSWER.to_owned(), headers: CallHeaders::new(), data: data.clone() };
        // Queued at the call; the send's wait is dropped unpolled.
        drop((outbound.send)(Pattern::from(ANSWER), request, Some(ReplyTo { id })));
    }
    let writing = Writing::start(&peer);
    gate("the server writes", || writing.written() > 0).await;

    let ended = Arc::new(AtomicBool::new(false));
    let closing = {
        let (client, ended) = (Arc::clone(&client), Arc::clone(&ended));
        tokio::spawn(async move {
            let closed = client.close().await;
            ended.store(true, Ordering::Release);
            closed
        })
    };
    let over = OVER_BUFFERS as u64 * buffered(&peer);
    let failed = writing.read_through("the client reads what the server writes once its close begins", over).await;
    assert!(failed.is_none(), "a write of the server failed as the client's close began: {failed:?}");
    assert!(!ended.load(Ordering::Acquire), "the client's close ended with its writer held");
    assert_eq!(clock.live(CUT_GRACE), 0, "the client's grace began before its close ended");

    clock.advance(CLOSE_TAKES);
    let read = read_all(&peer, "req");
    gate("the client's close ends once its writer is released", || ended.load(Ordering::Acquire)).await;
    tokio::time::timeout(PATIENCE, closing).await.expect("the close's task ends").expect("the close's task").expect("the client's link closes");
    let (read, end) = received("the server's read ends", &read).await;
    assert!(end.is_ok(), "the server's read after {read} requests ended {end:?}, not with the client's FIN");
    assert_eq!(read, requests, "the requests the server read");
    gate("the client asks the clock for its grace", || clock.live(CUT_GRACE) == 1).await;

    clock.advance(CUT_GRACE - Duration::from_millis(1));
    let failed = writing.read_through("the client reads what the server writes short of the grace", over).await;
    assert!(failed.is_none(), "a write of the server failed short of the grace from the close's end: {failed:?}");
    assert_eq!(clock.live(CUT_GRACE), 1, "the client's linger ended short of the grace");

    clock.advance(Duration::from_millis(1));
    let error = writing.fails("a write of the server fails once the grace has passed").await;
    assert!(reset(&error), "the server's write failed at the grace with {error}, not a reset");
    assert_eq!(clock.live(CUT_GRACE), 0, "the client's grace outlived its connection");
    drop(outbound);
}
