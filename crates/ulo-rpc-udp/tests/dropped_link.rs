//! A server's link dropped without `close`, as it is when its app drops before the close step,
//! releases its socket (F428): the address can be bound again. No datagram is sent: one the
//! receive loop reads after the inbound stream's drop ends that loop, which would release the
//! socket whether or not the drop reached the loop.
//!
//! A reply queued on the server's socket as its link drops is still sent: the link drops at the
//! end of an app's shutdown, right after a `close` cut by a spent `shutdown_timeout` has queued,
//! in its one poll, the answers of the calls the drain abandoned.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern};
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

// One thread: nothing runs the link's writer between the reply's queueing and the link's drop.
#[tokio::test(flavor = "current_thread")]
async fn a_reply_queued_as_the_server_link_drops_is_sent() {
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut udp = Udp::new("127.0.0.1:0");
    udp.prepare(&app.handle()).await.expect("the link prepares");
    let mut inbound = udp.listen(&[Pattern::from("dropped_link.add")]).await.expect("the link listens");
    let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");
    let caller = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("the caller's socket");
    let request = Frame::Req { id: 7, pattern: "dropped_link.add".to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
    caller.send_to(&Codec::Json.encode_frame(&request).expect("the request encodes"), addr).await.expect("the request is sent");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("the request in time").expect("a delivery");

    let reply = delivery.reply.expect("a request's reply path");
    // Queued at the call; the send's wait is dropped unpolled, as an aborted call's would be.
    drop(reply.send(Frame::Res { id: delivery.frame.id().expect("the request's id"), data: Data::new(b"2".as_slice()) }));
    drop(reply);
    drop(inbound);
    drop(udp);

    let mut buffer = vec![0u8; 65_536];
    let (len, from) = tokio::time::timeout(PATIENCE, caller.recv_from(&mut buffer)).await.expect("the reply in time").expect("the reply is received");
    assert_eq!(from, addr, "the reply came from the server's socket");
    let frame = Codec::Json.decode_frame(&buffer[..len]).expect("the reply decodes");
    assert_eq!(frame, Frame::Res { id: 7, data: Data::new(b"2".as_slice()) }, "the reply, under the caller's own id");
}
