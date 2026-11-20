//! A server's `close` returns with its socket closed, the link still held: its address binds again
//! at once, and a request a connected caller sends afterwards meets a closed port, which the OS
//! reports to the caller's next read (F588). A socket left open until the link drops would hold
//! such a request where nothing reads it, and its caller would meet its own `Timeout`. Planted
//! break: `close` leaving the writer running.

use std::net::UdpSocket as StdUdpSocket;
use std::time::Duration;

use bytes::Bytes;
use futures_util::StreamExt;
use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{CallHeaders, Codec, Data, Frame, Link, Pattern};
use ulo_rpc_udp::Udp;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_returns_with_the_socket_closed() {
    let app = App::builder(Root)
        .timer(ulo_runtime_tokio::Timer::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut udp = Udp::new("127.0.0.1:0");
    udp.prepare(&app.handle()).await.expect("the link prepares");
    let mut inbound = udp.listen(&[Pattern::from("close_releases.add").into()]).await.expect("the link listens");
    let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");

    let caller = StdUdpSocket::bind("127.0.0.1:0").expect("a loopback port");
    caller.connect(addr).expect("the caller aims at the server");
    caller.set_read_timeout(Some(Duration::from_secs(5))).expect("a read timeout");
    let request = |id| Frame::Req { id, pattern: "close_releases.add".to_owned(), headers: CallHeaders::new(), data: Data::new(Bytes::from_static(b"1")) };
    caller.send(&Codec::Json.encode_frame(&request(1)).expect("the request encodes")).expect("the request is sent");
    // The delivery's reply path is a sender of the writer's queue, held across `close`: a writer
    // nothing stops would wait on it, and keep the socket.
    let held = tokio::time::timeout(Duration::from_secs(5), inbound.next()).await.expect("the request is delivered").expect("a delivery");
    udp.close().await.expect("the link closes");
    // Read before anything else awaits, so a writer still ending after `close` returned holds it.
    let rebound = StdUdpSocket::bind(addr);
    assert!(rebound.is_ok(), "the closed link's address did not bind again as `close` returned: {rebound:?}");
    drop(rebound);

    let mut buffer = [0u8; 1024];
    let answered = caller.recv(&mut buffer).expect("the held call's `unavailable`");
    assert!(matches!(Codec::Json.decode_frame(&buffer[..answered]), Ok(Frame::Err { id: 1, .. })), "the held call was not answered at close");
    caller.send(&Codec::Json.encode_frame(&request(2)).expect("the request encodes")).expect("the request is sent");
    let read = caller.recv(&mut buffer);
    assert!(
        read.as_ref().is_err_and(|error| error.kind() == std::io::ErrorKind::ConnectionRefused),
        "a request sent after `close` did not meet a closed port: {read:?}"
    );

    drop((held, udp));
}
