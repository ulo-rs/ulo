//! A server flooding a client's connection with reply frames does not hold the connection's reply
//! lane open past the client's `close`: on smol a read answers at once while the peer's bytes keep
//! arriving, and so does a send to a reply lane the client keeps emptying, so a reply reader that
//! looks for the close only while a read is pending never sees it, and the call waiting on the
//! lane is not failed. The scenario runs on a one-thread executor beside a ticker, under a
//! watchdog on a thread of its own (`ulo-conformance-support`'s `starvation`), so a starved
//! executor fails the test naming the loop rather than hanging it.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_conformance_support::starvation::{Probe, Starvation};
use ulo_rpc::{Codec, Data, Frame, RpcClient, RpcClientModule};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;
use ulo_transport::ErrorKind;

/// Longer than any wait below, so the call is still waiting at the close.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

const LOOP: &str = "ulo-rpc-tcp-core reply reader (Reading::turn in Reading::frame, read by read_replies)";

/// What the peer has written when the reader is known to be inside the flood: three times what a
/// loopback connection takes with nothing reading it, about 1.7 MB on macOS and 2.7 MB on Linux, so
/// the reader has been taking it.
const FLOODED: u64 = 8 * 1024 * 1024;

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
fn flooding_peer(stop: Arc<AtomicBool>, written: Arc<AtomicU64>) -> (std::net::SocketAddr, mpsc::Receiver<()>) {
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
            written.fetch_add(batch.len() as u64, Ordering::Release);
        }
    });
    (addr, flood)
}

#[test]
fn a_flood_of_replies_does_not_hold_the_reply_lane_open_past_close() {
    Starvation::new(LOOP).run(run_a_flood_of_replies_does_not_hold_the_reply_lane_open_past_close);
}

async fn run_a_flood_of_replies_does_not_hold_the_reply_lane_open_past_close(probe: Probe) {
    let stop = Arc::new(AtomicBool::new(false));
    let written = Arc::new(AtomicU64::new(0));
    let (addr, flood) = flooding_peer(Arc::clone(&stop), Arc::clone(&written));
    let app = App::builder(ClientRoot { link: Mutex::new(Some(Tcp::new(addr))) })
        .runtime(OnSmol::runtime())
        .wire()
        .expect("the client app wires")
        .connect()
        .await
        .expect("the client app connects");
    let rpc = (*app.get::<RpcClient>().await.expect("the app holds an `RpcClient`")).clone();
    let call = ulo::spawn_with(&OnSmol::runtime(), async move { rpc.request::<String, String>("held", &"x".to_owned()).await });
    probe.until("the peer beginning its flood", probe.budget().boot, || flood.try_recv().is_ok()).await;
    probe.until("the reader taking the flood", probe.budget().boot, || written.load(Ordering::Acquire) > FLOODED).await;

    probe.within("the client app's close", probe.budget().boot, async {
        if let Err(error) = app.close(Signal::new("client_flood")).await {
            panic!("the client app's close reported a failure: {error}");
        }
    })
    .await;
    let answered = probe
        .within("the call waiting on the flooded reply lane failing at the close", probe.budget().recovery, call)
        .await
        .expect("the call's task ends");
    let error = answered.expect_err("the call fails at the close");
    assert_eq!(error.kind(), ErrorKind::Unavailable, "the call failed with {error}");
    stop.store(true, Ordering::Release);
}
