//! A client flooding an HTTP/2 connection with frames that need no answer does not hold the
//! server's close on smol: `async-io`'s sockets answer a read at once while the client's bytes
//! keep arriving, so a connection loop that awaits nothing else keeps its thread, and the abort
//! that ends the connection once the drain runs out never takes effect. The client is a plain
//! socket speaking h2c on a thread of its own, writing `WINDOW_UPDATE` frames for the connection,
//! and the scenario runs on a one-thread executor beside a ticker, under a watchdog on a thread of
//! its own (`ulo-conformance-support`'s `starvation`), so a starved executor fails the test naming
//! the loop rather than hanging it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;

use ulo::{Runtime, Signal};
use ulo_conformance_support::starvation::{Probe, Starvation};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_http_hyper::ServerOn;
use ulo_listener_smol::SmolListener;

/// The suite app's drain, past which the server aborts the connections still open.
const DRAIN: Duration = Duration::from_secs(4);

const LOOP: &str = "ulo-listener-smol read budget under hyper's HTTP/2 connection loop (SmolStream::poll_read yield every READS_PER_TURN reads)";

/// What the client has written when the connection's loop is known to be inside the flood: three
/// times what a loopback connection takes with nothing reading it, about 1.7 MB on macOS and 2.7 MB
/// on Linux, so the loop has been taking it.
const FLOODED: u64 = 8 * 1024 * 1024;

/// Opens an h2c connection and writes `WINDOW_UPDATE` frames of one byte for the connection until
/// a write fails or `stop` is set; answers the failure, if any, once the writing ends. A thread of
/// its own reads and discards what the server writes.
fn flooding_client(addr: std::net::SocketAddr, stop: Arc<AtomicBool>, written: Arc<AtomicU64>) -> (mpsc::Receiver<()>, mpsc::Receiver<Option<std::io::Error>>) {
    let (flooding, flood) = mpsc::channel();
    let (end, ended) = mpsc::channel();
    thread::spawn(move || {
        let mut stream = TcpStream::connect(addr).expect("the server accepts");
        let mut reading = stream.try_clone().expect("the client's socket clones");
        thread::spawn(move || {
            let mut buffer = vec![0u8; 64 * 1024];
            while matches!(reading.read(&mut buffer), Ok(read) if read > 0) {}
        });
        let mut preface = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
        preface.extend_from_slice(&[0, 0, 0, 4, 0, 0, 0, 0, 0]);
        stream.write_all(&preface).expect("the preface and SETTINGS are written");
        let updates: Vec<u8> = std::iter::repeat_n([0, 0, 4, 8, 0, 0, 0, 0, 0, 0, 0, 0, 1], 4_000).flatten().collect();
        let _ = flooding.send(());
        while !stop.load(Ordering::Acquire) {
            if let Err(error) = stream.write_all(&updates) {
                let _ = end.send(Some(error));
                return;
            }
            written.fetch_add(updates.len() as u64, Ordering::Release);
        }
        let _ = end.send(None);
    });
    (flood, ended)
}

#[test]
fn an_http2_frame_flood_does_not_hold_the_close() {
    Starvation::new(LOOP).allow(DRAIN * 3).run(run_an_http2_frame_flood_does_not_hold_the_close);
}

async fn run_an_http2_frame_flood_does_not_hold_the_close(probe: Probe) {
    let app = ulo_http_conformance::app(OnSmol::runtime()).await;
    let app = app.bind(ServerOn::<SmolListener>::new("127.0.0.1:0").h2c(true)).listen().await.expect("the app listens");
    let addr = app.addresses()[0].addr;
    let handle = app.handle();
    let runtime: Arc<dyn Runtime> = Arc::new(OnSmol::runtime());
    let serving = ulo::spawn_with(&*runtime, app.serve(std::future::pending::<Signal>()));
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let (flood, ended) = flooding_client(addr, Arc::clone(&stop), Arc::clone(&written));
    probe.until("the client beginning its flood", probe.budget().boot, || flood.try_recv().is_ok()).await;
    probe.until("the connection's loop taking the flood", probe.budget().boot, || written.load(Ordering::Acquire) > FLOODED).await;

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
