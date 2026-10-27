//! A server connection's `CUT_GRACE` runs from the end of the link's `close`, not from its start
//! (transports DESIGN §5.2, "A cut `close`"): the whole connection, a writer still holding answers
//! its peer has not read included, is held until the grace after `close` ends. The test makes the
//! `close` take a known time on the scripted clock, so a grace counted from its start would end the
//! connection while `close` is still running.
//!
//! The link runs through `Core` on a runtime that holds the first task aborted at `close` inside
//! its drop, which keeps `close` waiting for it; a thread of the test's own then moves the clock by
//! [`CLOSE_TAKES`], past `CUT_GRACE`, and lets the drop return. The hold is a blocked drop and not
//! an await because the abort of a task is the only wait `close` has, and a blocked thread cannot
//! be raced by the clock.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use async_net::TcpListener;
use futures_util::StreamExt;
use ulo::{BoxFuture, Spawn, TaskHandle, Timer};
use ulo_conformance_support::ScriptedTimer;
use ulo_rpc::{CallHeaders, Codec, Data, Frame};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_core::{Accept, CUT_GRACE, Core, Incoming};
use ulo_runtime_smol::Smol;

/// The size of each answer or request frame a test queues.
const FRAME_BYTES: usize = 64 * 1024;

/// How many times over the peer's granted buffers the server answers: more than the two sockets'
/// buffers hold.
const OVER_BUFFERS: usize = 16;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// How long the server's `close` takes on the scripted clock, past `CUT_GRACE`.
const CLOSE_TAKES: Duration = Duration::from_secs(3);

/// How long the clock's thread waits, the clock moved, before it reads whether `close` has ended:
/// time for a `close` the hold did not hold to end.
const UNHELD: Duration = Duration::from_millis(100);

fn buffered(peer: &TcpStream) -> u64 {
    let socket = socket2::SockRef::from(peer);
    let granted = socket.send_buffer_size().expect("the peer's SO_SNDBUF") + socket.recv_buffer_size().expect("the peer's SO_RCVBUF");
    granted as u64
}

/// How many frames of [`FRAME_BYTES`] the server answers on `peer`'s connection.
fn frames(peer: &TcpStream) -> u64 {
    (OVER_BUFFERS as u64 * buffered(peer)).div_ceil(FRAME_BYTES as u64)
}

async fn gate(what: &str, mut ready: impl FnMut() -> bool) {
    let began = Instant::now();
    while !ready() {
        assert!(began.elapsed() < PATIENCE, "{what}, not within {PATIENCE:?}");
        OnSmol::runtime().sleep(Duration::from_millis(5)).await;
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

/// Reads `res` frames from a thread of its own until the connection ends, answering how many it
/// read and how the read ended: `Ok` at a clean end between frames.
fn read_all(stream: &TcpStream) -> mpsc::Receiver<(u64, io::Result<()>)> {
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
            match Codec::Json.decode_frame(&body) {
                Ok(Frame::Res { .. }) => count += 1,
                Ok(Frame::Goaway | Frame::Cancel { .. }) => {}
                other => break Err(io::Error::other(format!("the link wrote a frame other than `res`: {other:?}"))),
            }
        });
        let _ = done.send((count, end));
    });
    read
}

/// Blocks the thread that drops the first unfinished task spawned through it, once armed, until
/// released.
struct Hold {
    armed: AtomicBool,
    entered: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

/// A runtime that spawns on smol and holds the first aborted task in its drop.
struct Holding {
    inner: Smol,
    hold: Arc<Hold>,
}

struct Held {
    task: BoxFuture<'static, ()>,
    done: bool,
    hold: Arc<Hold>,
}

impl std::future::Future for Held {
    type Output = ();

    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let polled = self.task.as_mut().poll(cx);
        self.done = polled.is_ready();
        polled
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if self.done || !self.hold.armed.swap(false, Ordering::AcqRel) {
            return;
        }
        let _ = self.hold.entered.lock().unwrap().send(());
        let _ = self.hold.release.lock().unwrap().recv_timeout(PATIENCE);
    }
}

impl Spawn for Holding {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.inner.spawn(Box::pin(Held { task: fut, done: false, hold: Arc::clone(&self.hold) }))
    }

    fn name(&self) -> &'static str {
        self.inner.name()
    }
}

impl Timer for Holding {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.inner.sleep(d)
    }

    fn now(&self) -> Instant {
        self.inner.now()
    }
}

struct Plain(TcpListener);

impl Accept for Plain {
    async fn accept(&mut self) -> io::Result<Incoming> {
        let (stream, peer) = self.0.accept().await?;
        Ok(Incoming::plain(peer, Box::pin(stream.clone()), Box::pin(stream)))
    }
}

/// The server answers every request with a frame of [`FRAME_BYTES`], reading no further than
/// the peer's requests while the peer reads none of the answers, so its writer is held mid-answer.
/// Its `close` takes [`CLOSE_TAKES`] on the clock the app handed, past the grace. The connection
/// is then held for the grace from the end of `close`, and dropped there.
#[test]
fn a_server_s_grace_runs_from_the_end_of_a_close_that_took_time() {
    OnSmol::block_on(run_a_server_s_grace_runs_from_the_end_of_a_close_that_took_time());
}

async fn run_a_server_s_grace_runs_from_the_end_of_a_close_that_took_time() {
    let clock = ScriptedTimer::new();
    let (entered, waiting) = mpsc::channel();
    let (released, release) = mpsc::channel();
    let hold = Arc::new(Hold { armed: AtomicBool::new(false), entered: Mutex::new(entered), release: Mutex::new(release) });

    let mut core = Core::new("127.0.0.1:0".into());
    core.prepare(Ok(()), Some(Arc::new(clock.clone()))).expect("the link prepares");
    let runtime = Arc::new(Holding { inner: OnSmol::runtime(), hold: Arc::clone(&hold) });
    let mut inbound = core
        .listen(runtime, |listener, _tls| Ok(Plain(TcpListener::try_from(listener.into_std())?)))
        .expect("the server listens");
    let addr = core.bound().first().map(|bound| bound.addr).expect("the server's address");

    let answered = Arc::new(AtomicU64::new(0));
    let counting = Arc::clone(&answered);
    drop(ulo::spawn_with(&OnSmol::runtime(), async move {
        while let Some(delivery) = inbound.next().await {
            let (Frame::Req { id, .. }, Some(reply)) = (delivery.frame, delivery.reply) else { continue };
            // Queued at the call; the send's wait is dropped unpolled.
            drop(reply.send(Frame::Res { id, data: Data::new(vec![b'a'; FRAME_BYTES]) }));
            counting.fetch_add(1, Ordering::SeqCst);
        }
    }));

    let mut peer = TcpStream::connect(addr).expect("the server accepts a connection");
    let calls = frames(&peer);
    for id in 1..=calls {
        let request = Frame::Req { id, pattern: "server_cut_grace".to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
        write_frame(&mut peer, &request).expect("the request is written");
    }
    gate("the server answers every call", || answered.load(Ordering::SeqCst) == calls).await;

    // Moves the clock once the drop is held, then records whether `close` had ended by `UNHELD`
    // later, and only then releases the hold.
    let ended = Arc::new(AtomicBool::new(false));
    let moving = {
        let (clock, ended) = (clock.clone(), Arc::clone(&ended));
        thread::spawn(move || {
            waiting.recv_timeout(PATIENCE).expect("an aborted task reached the hold");
            clock.advance(CLOSE_TAKES);
            thread::sleep(UNHELD);
            let ended_while_held = ended.load(Ordering::Acquire);
            released.send(()).expect("the hold waits for its release");
            ended_while_held
        })
    };
    hold.armed.store(true, Ordering::Release);
    let core = Arc::new(core);
    let closing = {
        let (core, ended) = (Arc::clone(&core), Arc::clone(&ended));
        ulo::spawn_with(&OnSmol::runtime(), async move {
            core.close().await;
            ended.store(true, Ordering::Release);
        })
    };
    gate("the server's close ends once its held task is released", || ended.load(Ordering::Acquire)).await;
    closing.await.expect("the close's task ends");
    assert!(!moving.join().expect("the clock's thread"), "the server's close ended with its task held");
    assert_eq!(clock.elapsed(), CLOSE_TAKES);

    gate("the connection asks the clock for its grace after the close's end", || {
        clock.live(CUT_GRACE) == 1 && clock.pending_at(CUT_GRACE) == Some(clock.elapsed())
    })
    .await;

    clock.advance(CUT_GRACE - Duration::from_millis(1));
    gate("the connection's grace sees the clock short of it", || clock.pending_at(CUT_GRACE) == Some(clock.elapsed())).await;
    assert_eq!(clock.live(CUT_GRACE), 1, "the connection was dropped short of the grace from the close's end");

    clock.advance(Duration::from_millis(1));
    gate("the grace drops the connection", || clock.live(CUT_GRACE) == 0).await;
    let (read, end) = received("the peer's read ends", &read_all(&peer)).await;
    assert!(
        end.as_ref().map_or_else(|error| error.kind() == ErrorKind::UnexpectedEof, |()| true),
        "the peer's read after {read} answers ended {end:?}, not with the server's FIN"
    );
    assert!(read < calls, "the peer read all {calls} answers: the writer outlived the grace");
    let _ = peer.shutdown(Shutdown::Both);
}
