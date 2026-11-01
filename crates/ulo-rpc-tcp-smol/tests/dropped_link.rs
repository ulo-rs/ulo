//! A server's link dropped without `close`, as it is when its app drops before the close step,
//! stops accepting (F428): its address refuses a connection and can be bound again, and a
//! connection it accepted before the drop reads its end. The accept loop holds the listener, and a
//! task's handle detaches on drop, so only a handle that aborts reaches it.

use std::io::{ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity, Timer};
use ulo_rpc::{Link, Pattern};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;

/// How long the drop has to show at the address.
const PATIENCE: Duration = Duration::from_secs(5);

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// Whether a connection to `addr` is refused. An open listener accepts it; a closed one answers
/// with a reset, which reads as `ConnectionRefused`.
fn refuses(addr: SocketAddr) -> bool {
    matches!(TcpStream::connect_timeout(&addr, Duration::from_millis(200)), Err(error) if error.kind() == ErrorKind::ConnectionRefused)
}

/// Whether `stream` has ended: its peer closed it, or reset it.
fn ended(stream: &mut TcpStream) -> bool {
    let mut byte = [0u8; 1];
    match stream.read(&mut byte) {
        Ok(0) => true,
        Ok(_) => false,
        Err(error) => !matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut),
    }
}

#[test]
fn a_server_link_dropped_without_close_releases_its_address() {
    OnSmol::block_on(dropped());
}

async fn dropped() {
    let app = App::builder(Root)
        .runtime(OnSmol::runtime())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut tcp = Tcp::new("127.0.0.1:0");
    tcp.prepare(&app.handle()).await.expect("the link prepares");
    let inbound = tcp.listen(&[Pattern::from("dropped_link.add")]).await.expect("the link listens");
    let addr = tcp.bound().first().map(|bound| bound.addr).expect("the link's address");

    let mut accepted = TcpStream::connect(addr).expect("the listening link accepts a connection");
    accepted.set_read_timeout(Some(Duration::from_millis(20))).expect("a read timeout");
    assert!(TcpListener::bind(addr).is_err(), "the address was bound again while the link listened");
    assert!(!ended(&mut accepted), "a connection the listening link accepted ended");

    drop(inbound);
    drop(tcp);
    let dropped = Instant::now();
    while !refuses(addr) {
        assert!(dropped.elapsed() < PATIENCE, "the address still accepts connections {PATIENCE:?} after the link's drop");
        OnSmol::runtime().sleep(Duration::from_millis(10)).await;
    }
    if let Err(error) = TcpListener::bind(addr) {
        panic!("the address refuses connections after the link's drop and cannot be bound again: {error}");
    }
    while !ended(&mut accepted) {
        assert!(dropped.elapsed() < PATIENCE, "a connection accepted before the link's drop is still open {PATIENCE:?} after it");
        OnSmol::runtime().sleep(Duration::from_millis(10)).await;
    }
}
