//! A server's link dropped without `close`, as it is when its app drops before the close step,
//! releases its socket (F428): the address can be bound again. No datagram is sent: one the
//! receive loop reads after the inbound stream's drop ends that loop, which would release the
//! socket whether or not the drop reached the loop.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{Link, Pattern};
use ulo_rpc_udp::Udp;

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

#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_without_close_releases_its_address() {
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut udp = Udp::new("127.0.0.1:0");
    udp.prepare(&app.handle()).await.expect("the link prepares");
    let inbound = udp.listen(&[Pattern::from("dropped_link.add")]).await.expect("the link listens");
    let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");
    assert!(UdpSocket::bind(addr).is_err(), "the address was bound again while the link listened");

    drop(inbound);
    drop(udp);
    let dropped = Instant::now();
    while let Err(error) = UdpSocket::bind(addr) {
        assert!(dropped.elapsed() < PATIENCE, "the address cannot be bound again {PATIENCE:?} after the link's drop: {error}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
