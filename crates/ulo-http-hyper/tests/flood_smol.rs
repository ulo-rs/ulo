//! A client flooding an HTTP/2 connection with frames that need no answer does not hold the
//! server's close on smol: `async-io`'s sockets answer a read at once while the client's bytes
//! keep arriving, so a connection loop that awaits nothing else keeps its thread, and the abort
//! that ends the connection once the drain runs out never takes effect. The client is a plain
//! socket speaking h2c on a thread of its own, writing `WINDOW_UPDATE` frames for the connection,
//! and the scenario runs on a thread of its own too, so a starved executor fails the test rather
//! than hanging it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use ulo::{Runtime, Signal};
use ulo_http_conformance::{Harness, OnSmol};
use ulo_http_hyper::ServerOn;
use ulo_listener_smol::SmolListener;

/// The suite app's drain, past which the server aborts the connections still open.
const DRAIN: Duration = Duration::from_secs(4);

/// How long the close and the connection's end may take past the drain.
const PATIENCE: Duration = Duration::from_secs(5);

/// Opens an h2c connection and writes `WINDOW_UPDATE` frames of one byte for the connection until
/// a write fails or `stop` is set; answers the failure, if any, once the writing ends. A thread of
/// its own reads and discards what the server writes.
fn flooding_client(addr: std::net::SocketAddr, stop: Arc<AtomicBool>) -> (mpsc::Receiver<()>, mpsc::Receiver<Option<std::io::Error>>) {
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
        }
        let _ = end.send(None);
    });
    (flood, ended)
}

#[test]
fn an_http2_frame_flood_does_not_hold_the_close() {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        OnSmol::block_on(run_an_http2_frame_flood_does_not_hold_the_close());
        let _ = done.send(());
    });
    match finished.recv_timeout(DRAIN + PATIENCE * 3) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the scenario failed; its panic is above"),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the scenario did not end: the flooded connection's task kept its thread"),
    }
}

async fn run_an_http2_frame_flood_does_not_hold_the_close() {
    let app = ulo_http_conformance::app(OnSmol::runtime()).await;
    let app = app.bind(ServerOn::<SmolListener>::new("127.0.0.1:0").h2c(true)).listen().await.expect("the app listens");
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
    // Long enough for the connection's loop to be spinning on the flood.
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
