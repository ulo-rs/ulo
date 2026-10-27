//! A server flooding a client's connection with reply frames does not hold the connection's reply
//! lane open past the client's `close`: on smol a read answers at once while the peer's bytes keep
//! arriving, and so does a send to a reply lane the client keeps emptying, so a reply reader that
//! looks for the close only while a read is pending never sees it, and the call waiting on the
//! lane is not failed. The scenario runs on a thread of its own, so a starved executor fails the
//! test rather than hanging it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, Timer};
use ulo_rpc::{Codec, Data, Frame, RpcClient, RpcClientModule};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;
use ulo_transport::ErrorKind;

/// Longer than any wait below, so the call is still waiting at the close.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the close has to show at the call.
const PATIENCE: Duration = Duration::from_secs(10);

struct ClientRoot {
    link: Mutex<Option<Tcp>>,
}

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.link.lock().unwrap().take() {
            m.import(RpcClientModule::for_root(link).timeout(Bound::After(CALL_TIMEOUT)));
        }
    }
}

/// Accepts one connection, reads the call's frame, then writes `res` frames naming no call until
/// a write fails or `stop` is set.
fn flooding_peer(stop: Arc<AtomicBool>) -> (std::net::SocketAddr, mpsc::Receiver<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the peer's address");
    let (flooding, flood) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("the client connects");
        let mut prefix = [0u8; 4];
        stream.read_exact(&mut prefix).expect("the call's frame arrives");
        let mut frame = vec![0u8; u32::from_be_bytes(prefix) as usize];
        stream.read_exact(&mut frame).expect("the call's frame arrives whole");
        let bytes = Codec::Json.encode_frame(&Frame::Res { id: u64::MAX, data: Data::new(b"1".as_slice()) }).expect("the frame encodes");
        let mut batch = Vec::new();
        for _ in 0..512 {
            batch.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
            batch.extend_from_slice(&bytes);
        }
        let _ = flooding.send(());
        while !stop.load(Ordering::Acquire) {
            if stream.write_all(&batch).is_err() {
                return;
            }
        }
    });
    (addr, flood)
}

#[test]
fn a_flood_of_replies_does_not_hold_the_reply_lane_open_past_close() {
    let (done, finished) = mpsc::channel();
    thread::spawn(move || {
        OnSmol::block_on(run_a_flood_of_replies_does_not_hold_the_reply_lane_open_past_close());
        let _ = done.send(());
    });
    match finished.recv_timeout(PATIENCE * 3) {
        Ok(()) => {}
        Err(mpsc::RecvTimeoutError::Disconnected) => panic!("the scenario failed; its panic is above"),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the scenario did not end within {:?}: the flooded reply reader kept its thread", PATIENCE * 3),
    }
}

async fn run_a_flood_of_replies_does_not_hold_the_reply_lane_open_past_close() {
    let stop = Arc::new(AtomicBool::new(false));
    let (addr, flood) = flooding_peer(Arc::clone(&stop));
    let app = App::builder(ClientRoot { link: Mutex::new(Some(Tcp::new(addr))) })
        .runtime(OnSmol::runtime())
        .wire()
        .expect("the client app wires")
        .connect()
        .await
        .expect("the client app connects");
    let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    let call = ulo::spawn_with(&OnSmol::runtime(), async move { rpc.request::<String, String>("held", &"x".to_owned()).await });
    let flooding = thread::spawn(move || flood.recv_timeout(PATIENCE).is_ok());
    while !flooding.is_finished() {
        OnSmol::runtime().sleep(Duration::from_millis(5)).await;
    }
    assert!(flooding.join().expect("the wait for the flood ends"), "the peer did not begin its flood within {PATIENCE:?}");
    // Long enough for the reader to be spinning on the flood.
    OnSmol::runtime().sleep(Duration::from_millis(200)).await;

    if let Err(error) = app.close(Signal::new("client_flood")).await {
        panic!("the client app's close reported a failure: {error}");
    }
    let expired = OnSmol::runtime().sleep(PATIENCE);
    let answered = match futures_util::future::select(call, expired).await {
        futures_util::future::Either::Left((answered, _)) => answered.expect("the call's task ends"),
        futures_util::future::Either::Right(_) => panic!("the call waiting on the flooded reply lane was not failed within {PATIENCE:?} of the close"),
    };
    let error = answered.expect_err("the call fails at the close");
    assert_eq!(error.kind(), ErrorKind::Unavailable, "the call failed with {error}");
    stop.store(true, Ordering::Release);
}
