//! A client whose runtime has shut down, while its link's own runtime lives, cannot bound a linger
//! when its reply lane drops: the clock `CUT_GRACE` is counted on is that runtime's. The
//! connection drops at once, without a panic, logged at `debug`, and is timed on no other clock.
//!
//! The server is a plain socket on threads of its own, reading the call's frame and then writing
//! `cancel` frames naming no call until a write fails: the client's drop resets it.

use std::fmt;
use std::io::{self, ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata, Subscriber};
use ulo::Runtime;
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern, ReplyTo};
use ulo_rpc_tcp::Tcp;
use ulo_rpc_tcp_core::CUT_GRACE;
use ulo_runtime_tokio::Tokio;

const HELD: &str = "dead_runtime.held";

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// What the link logs when the handed runtime refuses the grace.
const REFUSED: &str = "refused its grace";

/// Every panic in the test binary, counted by the hook `count_panics` installs. A task's panic is
/// caught at the task's boundary, so the hook is the one place it shows.
static PANICS: AtomicUsize = AtomicUsize::new(0);

fn count_panics() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANICS.fetch_add(1, Ordering::SeqCst);
            previous(info);
        }));
    });
}

/// The test binary's global `tracing` subscriber, keeping each event's fields as text. The link
/// logs from a task on a worker thread, which a thread-local subscriber would miss.
struct Recorder(Mutex<Vec<String>>);

impl Recorder {
    fn installed() -> &'static Recorder {
        static RECORDER: OnceLock<&'static Recorder> = OnceLock::new();
        RECORDER.get_or_init(|| {
            let recorder: &'static Recorder = Box::leak(Box::new(Recorder(Mutex::new(Vec::new()))));
            tracing::subscriber::set_global_default(Forward(recorder)).expect("the test binary installs no other subscriber");
            recorder
        })
    }

    fn containing(&self, needle: &str) -> usize {
        self.0.lock().unwrap().iter().filter(|text| text.contains(needle)).count()
    }
}

struct Forward(&'static Recorder);

impl Subscriber for Forward {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut text = Fields(format!("{} ", event.metadata().level()));
        event.record(&mut text);
        self.0.0.lock().unwrap().push(text.0);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

struct Fields(String);

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push_str(&format!("{}={value:?} ", field.name()));
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

/// Writes `cancel` frames naming no call from a thread of its own until a write fails, and answers
/// the failure.
fn writing(stream: &TcpStream) -> (Arc<AtomicU64>, mpsc::Receiver<io::Error>) {
    let mut stream = stream.try_clone().expect("the peer's socket clones");
    let mut batch = Vec::new();
    for _ in 0..512 {
        let bytes = Codec::Json.encode_frame(&Frame::Cancel { id: u64::MAX }).expect("the cancel encodes");
        batch.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        batch.extend_from_slice(&bytes);
    }
    let written = Arc::new(AtomicU64::new(0));
    let counting = Arc::clone(&written);
    let (end, ended) = mpsc::channel();
    thread::spawn(move || {
        loop {
            if let Err(error) = stream.write_all(&batch) {
                let _ = end.send(error);
                return;
            }
            counting.fetch_add(batch.len() as u64, Ordering::Release);
        }
    });
    (written, ended)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_whose_runtime_has_shut_down_drops_its_connection_at_once_without_a_panic() {
    count_panics();
    let recorder = Recorder::installed();
    let (addr, accepted) = serving_peer();
    let doomed = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .expect("the runtime the client is handed");
    let handed: Arc<dyn Runtime> = Arc::new(Tokio::from_handle(doomed.handle().clone()));
    let client = Tcp::new(addr);
    let outbound = client.connect(&handed).await.expect("the client connects");
    let request = Frame::Req { id: 1, pattern: HELD.to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
    (outbound.send)(Pattern::from(HELD), request, Some(ReplyTo { id: 1 })).await.expect("the request is written");
    let mut peer = None;
    gate("the call reaches the server", || {
        peer = accepted.try_recv().ok();
        peer.is_some()
    })
    .await;
    let peer = peer.expect("the gate opened on the connection");
    let (written, ended) = writing(&peer);
    gate("the server writes", || written.load(Ordering::Acquire) > 0).await;

    // Dropped on a thread of its own: dropping a runtime inside another's context panics.
    thread::spawn(move || drop(doomed)).join().expect("the client's runtime shuts down");
    let dropped = Instant::now();
    drop(outbound);

    let mut failed = None;
    gate("a write of the server fails once the client drops its reply lane", || {
        failed = ended.try_recv().ok();
        failed.is_some()
    })
    .await;
    let elapsed = dropped.elapsed();
    let error = failed.expect("the gate opened on a failure");
    assert!(
        matches!(error.kind(), ErrorKind::ConnectionReset | ErrorKind::BrokenPipe),
        "the server's write failed with {error}, not a reset"
    );
    assert_eq!(PANICS.load(Ordering::SeqCst), 0, "a panic was raised once the client's runtime had shut down");
    assert!(elapsed < CUT_GRACE / 2, "the connection lingered {elapsed:?} after the drop, on a clock other than the client's");
    gate("the link logs the refused grace at `debug`", || recorder.containing(REFUSED) == 1).await;
    let refusal = recorder.0.lock().unwrap().iter().find(|text| text.contains(REFUSED)).cloned();
    assert!(refusal.is_some_and(|text| text.starts_with("DEBUG ")), "the refused grace was logged at another level than `debug`");
    drop(client);
}
