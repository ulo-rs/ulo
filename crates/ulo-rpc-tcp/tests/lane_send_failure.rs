//! A client connection's reader that finds its reply lane gone as it hands a frame over, the
//! channel's receiver dropped while the reader holds a decoded frame, ends the lane as a dropped
//! lane (`LaneEnd::Dropped` at the send in `ulo-rpc-tcp-core`'s `read_replies`): the reader goes
//! to the connection's writer task, which discards what the peer still sends, and the grace
//! starts. The peer, still writing, reads the client's FIN and keeps writing. Ended as a lost
//! connection instead, the reader would drop with the peer's bytes unread, and the peer would read
//! a reset.
//!
//! The reply lane is dropped from inside the read that completes a chosen frame, so the reader's
//! next step is the send into a channel with no receiver: the lane's own drop signal, raised in
//! the same drop, is only read on the reader's next turn. The peer is a plain socket on threads
//! of its own. The clock is scripted and never advanced, so the grace never ends.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::task::{Context, Poll};
use std::thread;
use std::time::{Duration, Instant};

use futures_util::io::AsyncRead;
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use ulo::Runtime;
use ulo_conformance_support::ScriptedTimer;
use ulo_net::EndpointSpec;
use ulo_rpc::{Codec, Frame, Outbound};
use ulo_rpc_tcp_core::{CUT_GRACE, Core};
use ulo_runtime_tokio::Tokio;

/// The frame whose read drops the reply lane, counted from one.
const CUT_AT: u64 = 3;

/// How many times over the peer's granted buffers the peer writes after the cut: more than the
/// two sockets' buffers hold, so a connection that stopped reading fails a write.
const OVER_BUFFERS: u64 = 16;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

fn buffered(peer: &TcpStream) -> u64 {
    let socket = socket2::SockRef::from(peer);
    let granted = socket.send_buffer_size().expect("the peer's SO_SNDBUF") + socket.recv_buffer_size().expect("the peer's SO_RCVBUF");
    granted as u64
}

/// The client connection's reader: the socket's, dropping the connection's [`Outbound`] from
/// inside the read that completes frame [`CUT_AT`]. It follows the length-prefixed framing to know
/// where a frame ends; the core reads a prefix and then a body, each with `read_exact`, so the
/// read completing a body ends at that body's last byte.
struct Cutting<R> {
    inner: R,
    lane: Arc<Mutex<Option<Outbound>>>,
    prefix: [u8; 4],
    have: usize,
    body_left: usize,
    frames: u64,
}

impl<R> Cutting<R> {
    fn new(inner: R, lane: Arc<Mutex<Option<Outbound>>>) -> Self {
        Cutting { inner, lane, prefix: [0; 4], have: 0, body_left: 0, frames: 0 }
    }

    /// Follows `bytes` through the framing, answering whether frame `CUT_AT` ended in them.
    fn track(&mut self, mut bytes: &[u8]) -> bool {
        let mut cut = false;
        while !bytes.is_empty() {
            if self.body_left == 0 {
                let take = (4 - self.have).min(bytes.len());
                self.prefix[self.have..self.have + take].copy_from_slice(&bytes[..take]);
                self.have += take;
                bytes = &bytes[take..];
                if self.have == 4 {
                    self.have = 0;
                    self.body_left = u32::from_be_bytes(self.prefix) as usize;
                }
            } else {
                let take = self.body_left.min(bytes.len());
                self.body_left -= take;
                bytes = &bytes[take..];
                if self.body_left == 0 {
                    self.frames += 1;
                    cut |= self.frames == CUT_AT;
                }
            }
        }
        cut
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for Cutting<R> {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut [u8]) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        let read = match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(read)) => read,
            other => return other,
        };
        if this.track(&buf[..read]) {
            let lane = this.lane.lock().unwrap_or_else(PoisonError::into_inner).take();
            drop(lane.expect("the test placed the connection's lane before the peer wrote"));
        }
        Poll::Ready(Ok(read))
    }
}

/// Writes `cancel` frames naming no call from a thread of its own until a write fails or `stop`
/// is raised; `written` counts the bytes written, and the thread answers how it ended.
struct Writing {
    written: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    ended: mpsc::Receiver<io::Result<()>>,
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
        let stop = Arc::new(AtomicBool::new(false));
        let (end, ended) = mpsc::channel();
        let (counting, stopping) = (Arc::clone(&written), Arc::clone(&stop));
        thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                if let Err(error) = stream.write_all(&batch) {
                    let _ = end.send(Err(error));
                    return;
                }
                counting.fetch_add(batch.len() as u64, Ordering::Release);
            }
            let _ = end.send(Ok(()));
        });
        Writing { written, stop, ended }
    }

    fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
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

#[tokio::test(flavor = "multi_thread")]
async fn a_reply_lane_whose_send_fails_hands_its_reader_on_and_the_writing_peer_reads_a_fin() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the peer's address");
    let accepting = thread::spawn(move || listener.accept().expect("the client connects").0);
    let stream = tokio::net::TcpStream::connect(addr).await.expect("the client connects");
    let peer = accepting.join().expect("the peer accepts");

    let clock = ScriptedTimer::new();
    let lane = Arc::new(Mutex::new(None));
    let (reader, writer) = stream.into_split();
    let core = Core::new(EndpointSpec::from(addr.to_string().as_str()));
    let runtime: Arc<dyn Runtime> = Arc::new(Tokio::current());
    let outbound = core.connect(&runtime, Arc::new(clock.clone()), Box::pin(Cutting::new(reader.compat(), Arc::clone(&lane))), Box::pin(writer.compat_write()));
    *lane.lock().unwrap_or_else(PoisonError::into_inner) = Some(outbound);

    let (fin, read_fin) = mpsc::channel();
    let mut reading = peer.try_clone().expect("the peer's socket clones");
    thread::spawn(move || {
        let mut buffer = [0u8; 64];
        let _ = fin.send(reading.read(&mut buffer));
    });
    let writing = Writing::start(&peer);
    gate("the reply lane is dropped at its frame", || lane.lock().unwrap_or_else(PoisonError::into_inner).is_none()).await;

    let mut read = None;
    gate("the peer reads the client's end", || {
        read = read_fin.try_recv().ok();
        read.is_some()
    })
    .await;
    match read.expect("the gate opened on a read") {
        Ok(0) => {}
        Ok(n) => panic!("the peer read {n} bytes where the client's FIN belongs"),
        Err(error) => panic!("the peer read {error} where the client's FIN belongs, its connection reset"),
    }

    let (from, over) = (writing.written(), OVER_BUFFERS * buffered(&peer));
    let mut failed = None;
    gate("the client reads what the peer writes after its lane's send failed", || {
        failed = writing.ended.try_recv().ok();
        failed.is_some() || writing.written() > from + over
    })
    .await;
    if let Some(Err(error)) = failed {
        let kind = if matches!(error.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe) { "a reset" } else { "a failure" };
        panic!("a write of the peer met {kind} after the lane's send failed ({error}): the client stopped reading");
    }
    assert_eq!(clock.live(CUT_GRACE), 1, "the connection's grace did not start at the lane's end");

    writing.stop.store(true, Ordering::Release);
    // Polled, as the other waits here are: this future runs on one of the executor's threads, and a
    // blocking wait there stalls a task queued on that thread, which can be the connection's.
    let mut stopped = None;
    gate("the peer's writer stops", || {
        stopped = writing.ended.try_recv().ok();
        stopped.is_some()
    })
    .await;
    let stopped = stopped.expect("the gate opened on the writer's end");
    assert!(stopped.is_ok(), "the peer's last write failed: {stopped:?}");
    peer.shutdown(Shutdown::Write).expect("the peer shuts its half");
    gate("the connection ends once the peer has closed its half", || clock.live(CUT_GRACE) == 0).await;
}
