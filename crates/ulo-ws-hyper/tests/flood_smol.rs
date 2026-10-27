//! A client flooding Pong frames does not hold the standalone server's close on smol: `async-io`'s
//! sockets answer a read at once while the client's bytes keep arriving, and a Pong needs no
//! answer, so a read loop that never gives the executor a turn keeps its thread, and the abort
//! that ends the connection once the drain runs out never takes effect. The client is a plain
//! socket on a thread of its own, and the scenario runs on a thread of its own too, so a starved
//! executor fails the test rather than hanging it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity, Runtime, Signal, injectable, routes};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_listener_smol::SmolListener;
use ulo_ws::{Payload, WsModule};
use ulo_ws_hyper::ServerOn;

/// The app's drain: the server waits this long for the flooding client to answer its Close.
const DRAIN: Duration = Duration::from_secs(1);

/// How long the close and the connection's end may take past the drain.
const PATIENCE: Duration = Duration::from_secs(5);

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

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Echo>();
    }
}

/// Upgrades a connection to `/flood`, then writes empty masked Pong frames until a write fails or
/// `stop` is set; answers the failure, if any, once the writing ends.
fn flooding_client(addr: std::net::SocketAddr, stop: Arc<AtomicBool>) -> (mpsc::Receiver<()>, mpsc::Receiver<Option<std::io::Error>>) {
    let (flooding, flood) = mpsc::channel();
    let (end, ended) = mpsc::channel();
    thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("the server accepts");
        let request = format!(
            "GET /flood HTTP/1.1\r\nHost: {addr}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).expect("the upgrade request is written");
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).expect("the server answers the upgrade");
            head.push(byte[0]);
        }
        assert!(head.starts_with(b"HTTP/1.1 101"), "the server answered {:?}", String::from_utf8_lossy(&head));
        let pongs: Vec<u8> = std::iter::repeat_n([0x8A, 0x80, 1, 2, 3, 4], 20_000).flatten().collect();
        let _ = flooding.send(());
        while !stop.load(Ordering::Acquire) {
            if let Err(error) = stream.write_all(&pongs) {
                let _ = end.send(Some(error));
                return;
            }
        }
        let _ = end.send(None);
    });
    (flood, ended)
}

#[test]
fn a_pong_flood_does_not_hold_the_close() {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        OnSmol::block_on(run_a_pong_flood_does_not_hold_the_close());
        let _ = done.send(());
    });
    match finished.recv_timeout(DRAIN + PATIENCE * 3) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the scenario failed; its panic is above"),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the scenario did not end: the flooded connection's task kept its thread"),
    }
}

async fn run_a_pong_flood_does_not_hold_the_close() {
    let app = App::builder(Root)
        .runtime(OnSmol::runtime())
        .drain_timeout(DRAIN)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ServerOn::<SmolListener>::new("127.0.0.1:0"))
        .listen()
        .await
        .expect("the app listens");
    let addr = app.addresses()[0].addr;
    let handle = app.handle();
    let runtime: Arc<dyn Runtime> = Arc::new(OnSmol::runtime());
    let serving = ulo::spawn_with(&*runtime, app.serve(std::future::pending::<Signal>()));
    let stop = Arc::new(AtomicBool::new(false));
    let (flood, ended) = flooding_client(addr, Arc::clone(&stop));
    let began = Instant::now();
    while flood.try_recv().is_err() {
        assert!(began.elapsed() < PATIENCE, "the client did not begin its flood within {PATIENCE:?}");
        runtime.sleep(Duration::from_millis(5)).await;
    }
    // Long enough for the read loop to be spinning on the flood.
    runtime.sleep(Duration::from_millis(200)).await;

    let closing = ulo::spawn_with(&*runtime, async move { handle.close(Signal::new("flood")).await });
    let expired = runtime.sleep(DRAIN + PATIENCE);
    if let futures_util::future::Either::Right(_) = futures_util::future::select(closing, expired).await {
        panic!("the app's close did not end within {:?} of a {DRAIN:?} drain", DRAIN + PATIENCE);
    }
    let _ = serving.await;
    let began = Instant::now();
    let failed = loop {
        if let Ok(failed) = ended.try_recv() {
            break failed;
        }
        assert!(began.elapsed() < PATIENCE, "the client still wrote {PATIENCE:?} after the app closed: the server kept the connection");
        runtime.sleep(Duration::from_millis(5)).await;
    };
    stop.store(true, Ordering::Release);
    assert!(failed.is_some(), "the client's flood ended without a write failing");
}
