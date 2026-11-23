//! Q34-43 and Q34-52 through the link's own settings: an idle connection is sent `goaway` and
//! closed once `idle_timeout` passes on the app's clock, and the bounds a server cannot honour are
//! refused before it binds. The scenarios on a stepped executor live in `ulo-rpc-tcp-core`'s
//! `flow_tests`; these hold that the tokio link carries the settings to it.

use std::io::{ErrorKind, Read};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use ulo::testing::ScriptedTimer;
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, StartupError};
use ulo_rpc::config::Count;
use ulo_rpc::{Codec, Frame, Server};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

/// How long a gated wait lasts before the test fails.
const PATIENCE: Duration = Duration::from_secs(10);

const IDLE: Duration = Duration::from_secs(60);

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// Reads the frames `stream` carries until it ends, failing past `PATIENCE`.
fn read_to_end(stream: &mut TcpStream) -> Vec<Frame> {
    stream.set_read_timeout(Some(PATIENCE)).expect("the peer's read timeout sets");
    let mut bytes = Vec::new();
    if let Err(error) = stream.read_to_end(&mut bytes) {
        assert_ne!(error.kind(), ErrorKind::WouldBlock, "the connection was still open {PATIENCE:?} after its idle expiry");
        panic!("the connection failed: {error}");
    }
    let mut frames = Vec::new();
    let mut at = 0;
    while at + 4 <= bytes.len() {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("four bytes")) as usize;
        frames.push(Codec::Json.decode_frame(&bytes[at + 4..at + 4 + len]).expect("a frame the server wrote decodes"));
        at += 4 + len;
    }
    frames
}

/// Planted break: `Bound::Default` read as no timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_idle_connection_is_sent_goaway_and_closed() {
    let clock = ScriptedTimer::new();
    let server = App::builder(Root)
        .runtime(Tokio::current())
        .timer(clock.clone())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server listens");
    let addr = server.addresses()[0].addr;
    let handle = server.handle();
    let serving = tokio::spawn(server.serve(std::future::pending::<Signal>()));
    let mut peer = TcpStream::connect(addr).expect("the peer connects");
    let began = Instant::now();
    // The connection's handshake bound is a sleep of the same length, created before the idle
    // wait's and never polled on a plain socket. Advanced on a live sleep alone, the clock can move
    // before the connection reads it, and its idle wait, timed from 60 s, never ends.
    while clock.pending_at(IDLE).is_none() {
        assert!(began.elapsed() < PATIENCE, "the connection armed no idle wait within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    clock.advance(IDLE);
    let frames = tokio::task::spawn_blocking(move || read_to_end(&mut peer)).await.expect("the reader ran");
    assert_eq!(frames, vec![Frame::Goaway], "an idle connection is sent `goaway`, then the server's FIN");
    let _ = handle.close(Signal::new("test")).await;
    let _ = serving.await;
}

/// Planted break: `Tcp::max_connections` not reaching the core.
#[tokio::test(flavor = "current_thread")]
async fn bounds_a_server_cannot_honour_are_refused_before_it_binds() {
    let link = Tcp::new("127.0.0.1:0").idle_timeout(Bound::After(Duration::ZERO)).max_connections(Count::Max(0)).max_frame(100);
    let refused = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(Server::new(link))
        .listen()
        .await
        .err()
        .expect("a server with zero bounds listened");
    assert!(matches!(refused, StartupError::Configure(_)), "{refused:?}");
    let text = refused.to_string();
    for named in ["`.idle_timeout(Bound::After(Duration::ZERO))`", "`.max_connections(Count::Max(0))`", "`.max_frame(100)` is below 246 bytes"] {
        assert!(text.contains(named), "the refusal does not name {named}: {text}");
    }
}
