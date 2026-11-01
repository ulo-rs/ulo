use ex_24_ws_rooms::builder;
use ex_support::http::{Call, call};
use ex_support::start;
use ex_support::ws::{Socket, connect, hang_up, next, send};
use serde_json::json;

async fn member(addr: std::net::SocketAddr, room: Option<&str>) -> Socket {
    let mut socket = connect(addr, "/rooms", &[]).await;
    if let Some(room) = room {
        send(&mut socket, json!({ "event": "join", "id": 0, "data": room })).await;
        assert_eq!(next(&mut socket).await, json!({ "id": 0, "complete": true }), "the join is acknowledged");
    }
    socket
}

#[tokio::test]
async fn a_shout_reaches_every_member_of_its_room_and_no_one_outside_it() {
    let server = start(builder(None)).await;
    let mut ada = member(server.addr, Some("lobby")).await;
    let mut bob = member(server.addr, Some("lobby")).await;
    let mut cy = member(server.addr, Some("other")).await;
    let mut dee = member(server.addr, None).await;

    send(&mut ada, json!({ "event": "shout", "id": 1, "data": { "room": "lobby", "text": "hello" } })).await;

    let shouted = json!({ "event": "shouted", "data": "hello" });
    // A member sees the broadcast and the acknowledgement of its own call, in either order.
    let mut ada_frames = vec![next(&mut ada).await, next(&mut ada).await];
    ada_frames.sort_by_key(|frame| frame["id"].is_null());
    assert_eq!(ada_frames, vec![json!({ "id": 1, "complete": true }), shouted.clone()]);
    assert_eq!(next(&mut bob).await, shouted);

    // Anything headed for cy or dee would arrive ahead of the answer to a ping sent now.
    for (socket, id) in [(&mut cy, 5), (&mut dee, 6)] {
        send(socket, json!({ "event": "ping", "id": id })).await;
        assert_eq!(next(socket).await, json!({ "id": id, "data": "pong" }));
    }

    for socket in [ada, bob, cy, dee] {
        hang_up(socket).await;
    }
    server.stop().await.unwrap();
}

#[tokio::test]
async fn an_http_post_reaches_the_members_of_a_room_through_the_injected_rooms_and_no_one_outside_it() {
    let server = start(builder(None)).await;
    let mut ada = member(server.addr, Some("lobby")).await;
    let mut bob = member(server.addr, Some("lobby")).await;
    let mut cy = member(server.addr, Some("other")).await;
    let mut dee = member(server.addr, None).await;

    let reply = call(server.addr, Call::post("/notify/lobby").json(r#"{"text":"from http"}"#)).await;
    assert_eq!((reply.status, reply.body.as_str()), (200, "sent"));

    let notice = json!({ "event": "notice", "data": "from http" });
    assert_eq!(next(&mut ada).await, notice);
    assert_eq!(next(&mut bob).await, notice);

    // Anything headed for cy or dee would arrive ahead of the answer to a ping sent now.
    for (socket, id) in [(&mut cy, 5), (&mut dee, 6)] {
        send(socket, json!({ "event": "ping", "id": id })).await;
        assert_eq!(next(socket).await, json!({ "id": id, "data": "pong" }));
    }

    for socket in [ada, bob, cy, dee] {
        hang_up(socket).await;
    }
    server.stop().await.unwrap();
}
