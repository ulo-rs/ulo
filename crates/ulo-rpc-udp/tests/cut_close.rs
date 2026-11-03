//! A server's `close` cut after its first poll, its link then dropped, still answers each call it
//! held `err` of kind `unavailable` with the shutdown phrase (transports DESIGN §5.2, "A cut
//! `close`"). The core gives a cut `close` that one poll, and a call's own task may render the
//! drain's `unavailable` only once the link has dropped and its writer has ended, so the link
//! answers the calls itself, and a call's own answer after that poll is refused.

use std::collections::BTreeSet;
use std::pin::pin;
use std::task::{Context, Waker};
use std::time::Duration;

use futures_util::StreamExt;
use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{CallHeaders, Codec, Data, ErrorBody, Frame, Link, Pattern};
use ulo_rpc_udp::Udp;
use ulo_transport::{Details, ErrorKind, SHUTDOWN_MESSAGE};

/// How long the caller waits for each datagram.
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

fn request(id: u64) -> Frame {
    Frame::Req { id, pattern: "cut_close.hold".to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) }
}

// One thread: neither the writer nor the receive task runs between `close`'s one poll and the
// link's drop, so a held call's answer can only be one that poll queued. Two calls are held, so a
// `close` that waits for one answer's send before queueing the next is cut with the second unsent.
#[tokio::test(flavor = "current_thread")]
async fn the_calls_held_through_a_cut_close_are_answered_unavailable() {
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut udp = Udp::new("127.0.0.1:0");
    udp.prepare(&app.handle()).await.expect("the link prepares");
    let mut inbound = udp.listen(&[Pattern::from("cut_close.hold")]).await.expect("the link listens");
    let addr = udp.bound().first().map(|bound| bound.addr).expect("the link's address");
    let caller = tokio::net::UdpSocket::bind("127.0.0.1:0").await.expect("the caller's socket");
    let mut buffer = vec![0u8; 65_536];

    for id in [7, 8, 9] {
        caller.send_to(&Codec::Json.encode_frame(&request(id)).expect("the request encodes"), addr).await.expect("the request is sent");
    }
    let answered = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("the first request in time").expect("a delivery");
    let held = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("the second request in time").expect("a delivery");
    let also_held = tokio::time::timeout(PATIENCE, inbound.next()).await.expect("the third request in time").expect("a delivery");
    let answered_reply = answered.reply.expect("a request's reply path");
    let answered_id = answered.frame.id().expect("the request's id");
    answered_reply.send(Frame::Res { id: answered_id, data: Data::new(b"2".as_slice()) }).await.expect("the answer before the close is sent");
    let (len, _) = tokio::time::timeout(PATIENCE, caller.recv_from(&mut buffer)).await.expect("the answer in time").expect("the answer is received");
    let mut held_ids = BTreeSet::from([7, 8, 9]);
    match Codec::Json.decode_frame(&buffer[..len]).expect("the answer decodes") {
        Frame::Res { id, .. } if held_ids.remove(&id) => {}
        other => panic!("expected the answer to one of the three requests, got {other:?}"),
    }

    {
        let mut closing = pin!(udp.close());
        assert!(closing.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(), "the close ended in its first poll");
    }
    let reply = held.reply.expect("a request's reply path");
    let late = reply.send(Frame::Res { id: held.frame.id().expect("the request's id"), data: Data::new(b"3".as_slice()) });
    drop((inbound, also_held));
    drop(udp);
    assert!(late.await.is_err(), "the held call's own answer after the close's first poll was not refused");

    let mut answered_ids = BTreeSet::new();
    for _ in 0..2 {
        let (len, from) = tokio::time::timeout(PATIENCE, caller.recv_from(&mut buffer))
            .await
            .expect("a held call's answer in time")
            .expect("a held call's answer is received");
        assert_eq!(from, addr, "the answer came from the server's socket");
        let frame = Codec::Json.decode_frame(&buffer[..len]).expect("the answer decodes");
        let id = frame.id().expect("the answer's id");
        let unavailable = Frame::Err { id, error: ErrorBody::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE, Details::new()) };
        assert_eq!(frame, unavailable, "a held call's answer");
        answered_ids.insert(id);
    }
    assert_eq!(answered_ids, held_ids, "the held calls answered, each under its own id");
    let more = tokio::time::timeout(Duration::from_millis(200), caller.recv_from(&mut buffer)).await;
    assert!(!matches!(more, Ok(Ok(_))), "a datagram followed the held call's answer: {more:?}");
}
