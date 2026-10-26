//! A server's `close` lingers on each connection it ends (F484): the writer shuts its half once
//! the answers queued before the close are written, and the link reads and discards what the peer
//! still sends until the peer closes its half, the connection fails, or `CUT_GRACE` passes on the
//! app's clock. A socket closed with the peer's bytes unread closes with a reset, which discards
//! what the server had written and the peer had not yet read.
//!
//! The peer is a plain socket on threads of its own, writing `cancel` frames naming no call, which
//! the server reads and drops. The app's clock is scripted: no sleep it hands out ends until the
//! test advances it.

use std::collections::HashMap;
use std::future::Future;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll, Waker};
use std::thread;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ulo::{App, BoxFuture, Module, ModuleDef, ModuleIdentity, Timer};
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern};
use ulo_rpc_tcp::Tcp;
use ulo_rpc_tcp_core::CUT_GRACE;
use ulo_runtime_tokio::Tokio;

const ECHO: &str = "lingering_close.echo";

/// Answers queued before the close, each over 64 KiB: more than the two sockets' buffers hold
/// while the peer is not reading, so the last of them are still in the server's send buffer when
/// its socket closes.
const ANSWERS: u64 = 128;
const ANSWER_BYTES: usize = 64 * 1024;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// The app's clock: a sleep ends once the test has advanced the clock past it. Each live sleep is
/// counted by its length.
#[derive(Clone)]
struct Scripted(Arc<Mutex<Script>>);

struct Script {
    base: Instant,
    at: Duration,
    live: HashMap<Duration, usize>,
    waiting: Vec<Waker>,
}

struct Sleep {
    clock: Scripted,
    length: Duration,
    until: Duration,
}

impl Scripted {
    fn new() -> Scripted {
        Scripted(Arc::new(Mutex::new(Script { base: Instant::now(), at: Duration::ZERO, live: HashMap::new(), waiting: Vec::new() })))
    }

    fn advance(&self, by: Duration) {
        let waiting = {
            let mut script = self.0.lock().unwrap();
            script.at += by;
            std::mem::take(&mut script.waiting)
        };
        waiting.into_iter().for_each(Waker::wake);
    }

    fn live(&self, length: Duration) -> usize {
        self.0.lock().unwrap().live.get(&length).copied().unwrap_or(0)
    }
}

impl Timer for Scripted {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        let until = {
            let mut script = self.0.lock().unwrap();
            *script.live.entry(d).or_default() += 1;
            script.at + d
        };
        Box::pin(Sleep { clock: self.clone(), length: d, until })
    }

    fn now(&self) -> Instant {
        let script = self.0.lock().unwrap();
        script.base + script.at
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut script = self.clock.0.lock().unwrap();
        if script.at >= self.until {
            return Poll::Ready(());
        }
        script.waiting.push(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        let mut script = self.clock.0.lock().unwrap();
        if let Some(live) = script.live.get_mut(&self.length) {
            *live -= 1;
        }
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

fn write_frame(stream: &mut TcpStream, frame: &Frame) -> io::Result<()> {
    let bytes = Codec::Json.encode_frame(frame).expect("the frame encodes");
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)
}

/// Writes `cancel` frames naming no call from a thread of its own until `stop` is set or a write
/// fails; `written` counts the bytes written, and the thread answers the failure, if any.
struct Writing {
    stop: Arc<AtomicBool>,
    written: Arc<AtomicU64>,
    ended: mpsc::Receiver<Option<io::Error>>,
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
        let (stop, written) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let (end, ended) = mpsc::channel();
        let (stopping, counting) = (Arc::clone(&stop), Arc::clone(&written));
        thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                if let Err(error) = stream.write_all(&batch) {
                    let _ = end.send(Some(error));
                    return;
                }
                counting.fetch_add(batch.len() as u64, Ordering::Release);
            }
            let _ = end.send(None);
        });
        Writing { stop, written, ended }
    }

    fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// The write that failed, once the thread has ended on one.
    fn failed(&self) -> Option<io::Error> {
        self.ended.try_recv().ok().flatten()
    }

    /// Stops the thread and answers whether a write failed before it stopped.
    async fn stop(self) -> Option<io::Error> {
        self.stop.store(true, Ordering::Release);
        let mut ended = None;
        gate("the peer's writing thread ends", || match self.ended.try_recv() {
            Ok(end) => {
                ended = Some(end);
                true
            }
            Err(_) => false,
        })
        .await;
        ended.flatten()
    }
}

/// Reads frames from a thread of its own until the connection ends, answering the ids of the
/// `res` frames read and how the read ended: `Ok` at a clean end between frames, the server's FIN.
fn read_all(stream: &TcpStream) -> mpsc::Receiver<(Vec<u64>, io::Result<()>)> {
    let mut stream = stream.try_clone().expect("the peer's socket clones");
    stream.set_read_timeout(Some(PATIENCE / 2)).expect("a read timeout");
    let (done, read) = mpsc::channel();
    thread::spawn(move || {
        let mut ids = Vec::new();
        let end = loop {
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
            match Codec::Json.decode_frame(&body) {
                Ok(Frame::Res { id, .. }) => ids.push(id),
                other => break Err(io::Error::other(format!("the server wrote a frame other than `res`: {other:?}"))),
            }
        };
        let _ = done.send((ids, end));
    });
    read
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

/// A server link listening on the app's runtime and timing its linger with `clock`.
async fn listening(clock: &Scripted) -> (App<ulo::Connected>, Tcp, ulo_rpc::link::Inbound, SocketAddr) {
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .timer(clock.clone())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut tcp = Tcp::new("127.0.0.1:0");
    tcp.prepare(&app.handle()).await.expect("the link prepares");
    let inbound = tcp.listen(&[Pattern::from(ECHO)]).await.expect("the link listens");
    let addr = tcp.bound().first().map(|bound| bound.addr).expect("the link's address");
    (app, tcp, inbound, addr)
}

fn request(id: u64) -> Frame {
    Frame::Req { id, pattern: ECHO.to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) }
}

/// Sends `ANSWERS` requests and queues an answer to each at the server, none of them read yet.
async fn queue_answers(peer: &mut TcpStream, inbound: &mut ulo_rpc::link::Inbound) {
    for id in 1..=ANSWERS {
        write_frame(peer, &request(id)).expect("the request is written");
    }
    let answer = Data::new(format!("\"{}\"", "a".repeat(ANSWER_BYTES)).into_bytes());
    for _ in 0..ANSWERS {
        let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("a request in time").expect("a delivery");
        let id = delivery.frame.id().expect("a request's id");
        // Queued at the call; the send's wait is dropped unpolled.
        drop(delivery.reply.expect("a request's reply path").send(Frame::Res { id, data: answer.clone() }));
    }
}

/// Waits for a write of `writing` to fail, and answers the failure.
async fn write_fails(what: &str, writing: &Writing) -> io::Error {
    let mut failed = None;
    gate(what, || {
        failed = writing.failed();
        failed.is_some()
    })
    .await;
    failed.expect("the gate opened on a failure")
}

/// Whether `error` is what a write meets on a connection its peer dropped with bytes unread.
fn reset(error: &io::Error) -> bool {
    matches!(error.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_writing_through_close_reads_every_queued_answer_then_a_fin() {
    let clock = Scripted::new();
    let (_app, tcp, mut inbound, addr) = listening(&clock).await;
    let mut peer = TcpStream::connect(addr).expect("the link accepts a connection");

    queue_answers(&mut peer, &mut inbound).await;
    let writing = Writing::start(&peer);
    gate("the peer writes", || writing.written() > 0).await;

    tcp.close().await.expect("the link closes");
    let (ids, end) = received("the peer's read ends", &read_all(&peer)).await;
    assert!(end.is_ok(), "the peer's read after {} answers ended {end:?}, not with the server's FIN", ids.len());
    assert_eq!(ids, (1..=ANSWERS).collect::<Vec<_>>(), "the answers the peer read");
    let failed = writing.stop().await;
    assert!(failed.is_none(), "a write of the peer still writing through the close failed: {failed:?}");

    peer.shutdown(Shutdown::Write).expect("the peer shuts its half");
    gate("the linger ends at the peer's FIN", || clock.live(CUT_GRACE) == 0).await;
    drop(inbound);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_never_stops_writing_is_dropped_at_the_grace() {
    let clock = Scripted::new();
    let (_app, tcp, mut inbound, addr) = listening(&clock).await;
    let mut peer = TcpStream::connect(addr).expect("the link accepts a connection");
    // A delivery shows the server's connection task reading before the close.
    write_frame(&mut peer, &request(1)).expect("the request is written");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("a request in time").expect("a delivery");
    drop(delivery);
    let writing = Writing::start(&peer);
    gate("the peer writes", || writing.written() > 0).await;

    tcp.close().await.expect("the link closes");
    let (ids, end) = received("the peer's read ends", &read_all(&peer)).await;
    assert!(ids.is_empty(), "the server answered {ids:?}");
    assert!(end.is_ok(), "the peer's read ended {end:?}, not with the server's FIN");
    gate("the link asks the app's clock for its grace", || clock.live(CUT_GRACE) == 1).await;

    clock.advance(CUT_GRACE - Duration::from_millis(1));
    let before = writing.written();
    gate("the linger reads what the peer writes short of the grace", || writing.written() > before + 4 * 1024 * 1024).await;
    assert!(writing.failed().is_none(), "the peer's write failed short of the grace");
    assert_eq!(clock.live(CUT_GRACE), 1, "the linger ended short of the grace");

    clock.advance(Duration::from_millis(1));
    let error = write_fails("a write of the peer fails once the grace has passed", &writing).await;
    assert!(reset(&error), "the peer's write failed at the grace with {error}, not a reset");
    assert_eq!(clock.live(CUT_GRACE), 0, "the linger's sleep outlived the connection");
    drop(inbound);
}

/// The writer, held by a peer reading none of its answers, is stopped at the grace too: the
/// connection drops though answers are still queued.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_neither_reads_nor_stops_writing_is_dropped_at_the_grace() {
    let clock = Scripted::new();
    let (_app, tcp, mut inbound, addr) = listening(&clock).await;
    let mut peer = TcpStream::connect(addr).expect("the link accepts a connection");
    queue_answers(&mut peer, &mut inbound).await;
    let writing = Writing::start(&peer);
    gate("the peer writes", || writing.written() > 0).await;

    tcp.close().await.expect("the link closes");
    gate("the link asks the app's clock for its grace", || clock.live(CUT_GRACE) == 1).await;
    assert!(writing.failed().is_none(), "the peer's write failed short of the grace");
    clock.advance(CUT_GRACE);
    let error = write_fails("a write of the peer fails once the grace has passed", &writing).await;
    assert!(reset(&error), "the peer's write failed at the grace with {error}, not a reset");
    drop(inbound);
}

/// A connection the link drops on a frame that does not decode does not linger: the peer still
/// writing meets the reset with the app's clock never moved.
#[tokio::test(flavor = "multi_thread")]
async fn a_frame_that_does_not_decode_drops_the_connection_without_a_linger() {
    let clock = Scripted::new();
    let (_app, _tcp, inbound, addr) = listening(&clock).await;
    let mut peer = TcpStream::connect(addr).expect("the link accepts a connection");
    let garbage = b"not a frame";
    peer.write_all(&(garbage.len() as u32).to_be_bytes()).expect("the prefix is written");
    peer.write_all(garbage).expect("the frame is written");
    let writing = Writing::start(&peer);

    let error = write_fails("a write of the peer fails after the frame that does not decode", &writing).await;
    assert!(reset(&error), "the peer's write failed with {error}, not a reset");
    assert_eq!(clock.live(CUT_GRACE), 0, "the dropped connection lingered");
    drop(inbound);
}
