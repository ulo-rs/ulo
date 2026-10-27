//! A client dropped without `close` ends each connection it opened as `close` does: the writer
//! shuts its half once the frames queued before the drop are written, and the link reads and
//! discards what the server still sends until the server closes its half or the connection fails,
//! for at most `CUT_GRACE` from the drop on the clock of the runtime the client hands `connect`.
//! A socket closed with the server's bytes unread would reset a server still writing. Three drops
//! reach it: a client app's, a connection's reply lane's with its link kept, and the link's with
//! the connection's lane kept.
//!
//! The server is a plain socket on threads of its own, reading the call's frame and then writing
//! `cancel` frames naming no call. The clock is scripted: no sleep it hands out ends until the
//! test advances it.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ulo::{App, Bound, BoxFuture, Module, ModuleDef, ModuleIdentity, Runtime, Spawn, TaskHandle, Timer};
use ulo_conformance_support::ScriptedTimer;
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern, ReplyTo, RpcClient, RpcClientModule};
use ulo_rpc_tcp::Tcp;
use ulo_rpc_tcp_core::CUT_GRACE;
use ulo_runtime_tokio::Tokio;

const HELD: &str = "dropped_client.held";

/// How many times over the peer's granted buffers the server writes while the test waits for the
/// link to read: more than the two sockets' buffers hold.
const OVER_BUFFERS: u64 = 16;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// The bytes the kernel granted the peer's send and receive buffers, read back: the stand-in for
/// the link's socket at the other end, which the test cannot reach.
fn buffered(peer: &TcpStream) -> u64 {
    let socket = socket2::SockRef::from(peer);
    let granted = socket.send_buffer_size().expect("the peer's SO_SNDBUF") + socket.recv_buffer_size().expect("the peer's SO_RCVBUF");
    granted as u64
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
}

impl<S: Spawn> Spawn for Handed<S> {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawn.spawn(fut)
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

/// A server that accepts one connection on a thread of its own, reads the frame the client's
/// call writes, and hands the connection over.
fn serving_peer() -> (SocketAddr, mpsc::Receiver<TcpStream>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the peer's address");
    let (accepted, connection) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the client connects");
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix).expect("the call's frame arrives");
        let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut frame).expect("the call's frame arrives whole");
        let _ = accepted.send(stream);
    });
    (addr, connection)
}

/// After the drop, the server writing on `peer` is read until `CUT_GRACE` on `clock` has passed,
/// and reset there.
async fn read_until_the_grace(clock: &ScriptedTimer, peer: &TcpStream, writing: &Writing) {
    let over = OVER_BUFFERS * buffered(peer);
    let failed = writing.read_through("the client reads what the server writes after the drop", over).await;
    assert!(failed.is_none(), "a write of the server failed at the drop: {failed:?}");
    gate("the client asks the clock for its grace", || clock.live(CUT_GRACE) == 1).await;

    clock.advance(CUT_GRACE - Duration::from_millis(1));
    let failed = writing.read_through("the client reads what the server writes short of the grace", over).await;
    assert!(failed.is_none(), "a write of the server failed short of the grace: {failed:?}");
    assert_eq!(clock.live(CUT_GRACE), 1, "the client's linger ended short of the grace");

    clock.advance(Duration::from_millis(1));
    let error = writing.fails("a write of the server fails once the grace has passed").await;
    assert!(reset(&error), "the server's write failed at the grace with {error}, not a reset");
    assert_eq!(clock.live(CUT_GRACE), 0, "the client's grace outlived its connection");
}

/// A client link's connection with the server writing on it: the link, its connection and the
/// server's socket.
async fn writing_server(clock: &ScriptedTimer) -> (Tcp, ulo_rpc::Outbound, TcpStream, Writing) {
    let (addr, accepted) = serving_peer();
    let handed: Arc<dyn Runtime> = Arc::new(Handed { clock: clock.clone(), spawn: Tokio::current() });
    let client = Tcp::new(addr);
    let outbound = client.connect(&handed).await.expect("the client connects");
    let request = Frame::Req { id: 1, pattern: HELD.to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
    (outbound.send)(Pattern::from(HELD), request, Some(ReplyTo { id: 1 })).await.expect("the request is written");
    let peer = received("the call reaches the server", &accepted).await;
    let writing = Writing::start(&peer);
    gate("the server writes", || writing.written() > 0).await;
    (client, outbound, peer, writing)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_whose_reply_lane_is_dropped_reads_a_writing_server_until_the_grace() {
    let clock = ScriptedTimer::new();
    let (client, outbound, peer, writing) = writing_server(&clock).await;
    drop(outbound);
    read_until_the_grace(&clock, &peer, &writing).await;
    drop(client);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_dropped_without_close_reads_a_writing_server_until_the_grace() {
    let clock = ScriptedTimer::new();
    let (client, outbound, peer, writing) = writing_server(&clock).await;
    drop(client);
    read_until_the_grace(&clock, &peer, &writing).await;
    drop(outbound);
}

/// Imports one `RpcClientModule` over the link it is given.
struct ClientRoot(Mutex<Option<Tcp>>);

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.0.lock().unwrap().take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::Unbounded));
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_app_dropped_without_close_reads_a_writing_server_until_the_grace() {
    let clock = ScriptedTimer::new();
    let (addr, accepted) = serving_peer();
    let app = App::builder(ClientRoot(Mutex::new(Some(Tcp::new(addr)))))
        .runtime(Tokio::current())
        .timer(clock.clone())
        .wire()
        .expect("the client app wires")
        .connect()
        .await
        .expect("the client app connects");
    let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    let call = tokio::spawn(async move { rpc.request::<String, String>(HELD, &"x".to_owned()).await });
    let peer = received("the call reaches the server", &accepted).await;
    let writing = Writing::start(&peer);
    gate("the server writes", || writing.written() > 0).await;

    // The call's task holds the last `RpcClient` outside the app.
    call.abort();
    let _ = call.await;
    drop(app);
    read_until_the_grace(&clock, &peer, &writing).await;
}
