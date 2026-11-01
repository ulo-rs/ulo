//! The WebSocket server's loops that read a peer's frames give the executor a turn every 32 turns
//! (`TURNS_PER_YIELD`), whatever the frames are (transports DESIGN §3.7). A peer's frames that
//! need no answer, Pongs among them, are all a read loop meets while they keep arriving, and a loop
//! that never yields keeps the thread from every other task, the server's abort of the connection
//! among them.
//!
//! Each case runs on a one-thread executor beside a ticker. The peer writes the upgrade request
//! and the whole flood in one write, and the listener below holds back its first read until the
//! write has ended, so the flood is in the kernel before the server's loop first polls and the
//! loop never meets a read that would wait. The listener hands out at most [`CAP`] bytes a read,
//! so the bytes it has handed over count the Pongs the loop has taken to within one read, and the
//! ticker samples that count each turn. The largest advance between two turns, with the loop
//! yielding every 32 Pongs, is 32 Pongs and the reads around a turn; without a yield it is the whole flood.

use std::cell::RefCell;
use std::io;
use std::net::{SocketAddr, TcpStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use async_net::TcpListener;
use futures_io::{AsyncRead, AsyncWrite};
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_conformance_support::starvation::{Flood, Meter, Probe, Starvation};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;
use ulo_server_hyper::{FuturesIo, Incoming, Listener};
use ulo_ws::{ConnectCx, ConnectRefused, OnConnect, Payload, WsModule};
use ulo_ws_hyper::ServerOn;

/// The most bytes one read of the server's socket hands over: ten empty masked Pongs.
const CAP: usize = 60;

/// A masked Pong with no payload.
const PONG: [u8; 6] = [0x8A, 0x80, 1, 2, 3, 4];

/// How many Pongs a peer writes: far more than the kernel's buffers hold back from a loop that
/// reads them one `CAP` at a time, few enough that the single write needs no reader.
const PONGS: usize = 3_000;

/// The Pongs a loop takes between two of its yields, and the two reads that can be in flight
/// around a turn of the ticker: the most the meter may advance between two turns.
///
/// The two reads are 120 bytes, 20 Pongs, so the bound tells a loop yielding every 64 turns from
/// one yielding every 32, and cannot tell one yielding every 48 from every 32: the resolution is
/// set by `CAP`, since each read can move the meter by up to 60 bytes.
const ALLOWED_GAP: u64 = 32 * PONG.len() as u64 + 2 * CAP as u64;

/// Holds back the first read of every connection the listener accepts on this thread until
/// [`release`](Self::release), and counts what is read after.
struct Hold {
    open: AtomicBool,
    waker: Mutex<Option<Waker>>,
}

impl Hold {
    fn release(&self) {
        self.open.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().unwrap_or_else(PoisonError::into_inner).take() {
            waker.wake();
        }
    }
}

thread_local! {
    /// What the listener on this thread, the case's executor thread, accepts with.
    static HELD: RefCell<Option<(Arc<Hold>, Meter)>> = const { RefCell::new(None) };
}

struct Capped {
    inner: async_net::TcpStream,
    hold: Arc<Hold>,
    meter: Meter,
}

impl AsyncRead for Capped {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        if !this.hold.open.load(Ordering::Acquire) {
            *this.hold.waker.lock().unwrap_or_else(PoisonError::into_inner) = Some(cx.waker().clone());
            if !this.hold.open.load(Ordering::Acquire) {
                return Poll::Pending;
            }
        }
        let room = buf.len().min(CAP);
        let read = match Pin::new(&mut this.inner).poll_read(cx, &mut buf[..room]) {
            Poll::Ready(Ok(read)) => read,
            other => return other,
        };
        this.meter.add(read as u64);
        Poll::Ready(Ok(read))
    }
}

impl AsyncWrite for Capped {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_close(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_close(cx)
    }
}

/// A plain smol listener without the read budget the real one carries, so the loops under test
/// are the only yield the flood meets.
struct Held {
    tcp: TcpListener,
}

impl Listener for Held {
    type Io = FuturesIo<Capped>;

    fn adopt(listener: BoundListener, _tls: Option<Arc<ServerConfig>>) -> io::Result<Self> {
        Ok(Held { tcp: TcpListener::try_from(listener.into_std())? })
    }

    async fn accept(&self) -> io::Result<Incoming<Self::Io>> {
        let (inner, peer) = self.tcp.accept().await?;
        let local = inner.local_addr().ok();
        let (hold, meter) = HELD.with(|held| held.borrow().clone()).expect("the case installed its hold on this thread");
        Ok(Incoming::plain(FuturesIo::new(Capped { inner, hold, meter }), peer, local))
    }
}

#[injectable]
struct Echo;

#[routes]
#[ulo_ws::gateway(path = "/flood", port = own)]
impl Echo {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

/// Refuses every connection once the upgrade is done, so the refusal's loop is what the peer's
/// frames meet.
#[injectable]
struct Refusing;

#[routes]
#[ulo_ws::gateway(path = "/refuse", port = own)]
impl Refusing {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

impl OnConnect for Refusing {
    async fn on_connect(&self, _cx: &ConnectCx) -> Result<(), ConnectRefused> {
        Err(ConnectRefused::kind(ulo_transport::ErrorKind::Forbidden, "refused"))
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Echo>();
        m.controller::<Refusing>();
    }
}

/// A server on the held listener with a pong timeout of 20 s, the refusal's wait for the peer.
async fn serve(probe: &Probe, hold: &Arc<Hold>) -> (SocketAddr, impl Sized) {
    HELD.with(|held| *held.borrow_mut() = Some((Arc::clone(hold), probe.meter())));
    let app = App::builder(Root)
        .runtime(probe.runtime())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ServerOn::<Held>::new("127.0.0.1:0").pong_timeout(Bound::After(Duration::from_secs(20))))
        .listen()
        .await
        .expect("the app listens");
    let addr = app.addresses()[0].addr;
    let serving = ulo::spawn_with(&probe.runtime(), app.serve(std::future::pending::<Signal>()));
    (addr, serving)
}

/// The upgrade request to `path` and `PONGS` Pongs behind it, to be written in one write.
fn request(addr: SocketAddr, path: &str) -> Vec<u8> {
    let mut bytes = format!(
        "GET {path} HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
    )
    .into_bytes();
    bytes.extend(std::iter::repeat_n(PONG, PONGS).flatten());
    bytes
}

/// Writes `request` while the listener holds back its reads, releases them, and waits until the
/// server has read every byte; the largest advance of the meter between two turns of the ticker
/// is then the measure.
async fn flooded(probe: &Probe, path: &str) {
    let hold = Arc::new(Hold { open: AtomicBool::new(false), waker: Mutex::new(None) });
    let (addr, _serving) = serve(probe, &hold).await;
    let peer = TcpStream::connect(addr).expect("the server accepts");
    let bytes = request(addr, path);
    let total = bytes.len() as u64;
    let write = Flood::once(&peer, bytes);
    probe.settles("the peer's single write of the upgrade and the flood ending", || write.ended()).await;
    assert!(write.failure().is_none(), "the peer's write failed");
    probe.ticks_advance(1).await;
    probe.reset_gap();
    hold.release();
    probe.settles(&format!("the server reading all {total} bytes of the upgrade and the flood"), || probe.metered() == total).await;
    probe.expect_gap_within(ALLOWED_GAP, "bytes");
    drop(write);
}

const SERVE_LOOP: &str = "ulo-ws read loop (serve, yield_now every TURNS_PER_YIELD turns)";

#[test]
fn a_flood_of_pongs_buffered_before_the_loop_starts_does_not_hold_the_executor() {
    Starvation::new(SERVE_LOOP).run(|probe| async move { flooded(&probe, "/flood").await });
}

const REFUSE_LOOP: &str = "ulo-ws refuse loop (the wait for the peer's Close after a refusal)";

#[test]
fn a_flood_of_pongs_during_a_refusal_does_not_hold_the_executor() {
    Starvation::new(REFUSE_LOOP).run(|probe| async move { flooded(&probe, "/refuse").await });
}
