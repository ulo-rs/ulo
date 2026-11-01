//! A client flooding Pong frames does not hold the standalone server's close on smol: `async-io`'s
//! sockets answer a read at once while the client's bytes keep arriving, and a Pong needs no
//! answer, so a read loop that never gives the executor a turn keeps its thread, and the abort
//! that ends the connection once the drain runs out never takes effect. The client is a plain
//! socket on a thread of its own, and the scenario runs on a one-thread executor beside a ticker,
//! under a watchdog on a thread of its own (`ulo-conformance-support`'s `starvation`), so a starved
//! executor fails the test naming the loop rather than hanging it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use ulo::{App, Module, ModuleDef, ModuleIdentity, Runtime, Signal, injectable, routes};
use ulo_conformance_support::starvation::{Probe, Starvation};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_listener_smol::SmolListener;
use ulo_ws::{Payload, WsModule};
use ulo_ws_hyper::ServerOn;

/// The app's drain: the server waits this long for the flooding client to answer its Close.
const DRAIN: Duration = Duration::from_secs(1);

const LOOP: &str = "ulo-ws read loop on ulo-listener-smol (serve, yield_now every TURNS_PER_YIELD turns)";

/// What the client has written when the connection's loop is known to be inside the flood: three
/// times what a loopback connection takes with nothing reading it, about 1.7 MB on macOS and 2.7 MB
/// on Linux, so the loop has been taking it.
const FLOODED: u64 = 8 * 1024 * 1024;

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
fn flooding_client(addr: std::net::SocketAddr, stop: Arc<AtomicBool>, written: Arc<AtomicU64>) -> (mpsc::Receiver<()>, mpsc::Receiver<Option<std::io::Error>>) {
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
            written.fetch_add(pongs.len() as u64, Ordering::Release);
        }
        let _ = end.send(None);
    });
    (flood, ended)
}

#[test]
fn a_pong_flood_does_not_hold_the_close() {
    Starvation::new(LOOP).allow(DRAIN * 3).run(run_a_pong_flood_does_not_hold_the_close);
}

async fn run_a_pong_flood_does_not_hold_the_close(probe: Probe) {
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
    let written = Arc::new(AtomicU64::new(0));
    let (flood, ended) = flooding_client(addr, Arc::clone(&stop), Arc::clone(&written));
    probe.until("the client beginning its flood", probe.budget().boot, || flood.try_recv().is_ok()).await;
    probe.until("the read loop taking the flood", probe.budget().boot, || written.load(Ordering::Acquire) > FLOODED).await;

    let closing = ulo::spawn_with(&*runtime, async move { handle.close(Signal::new("flood")).await });
    probe.within(&format!("the app's close, a {DRAIN:?} drain included"), DRAIN + probe.budget().recovery, closing).await.expect("the close task ends").expect("the app closes");
    let _ = serving.await;
    let mut failed = None;
    probe
        .until("the client's write failing once the app has closed: the server kept the connection", probe.budget().recovery, || {
            failed = ended.try_recv().ok();
            failed.is_some()
        })
        .await;
    stop.store(true, Ordering::Release);
    assert!(failed.flatten().is_some(), "the client's flood ended without a write failing");
}
