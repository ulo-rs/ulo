use std::time::Duration;

use ex_23_ws_gateway::{DRAIN, Journal, builder};
use ex_support::ws::{close_frame, connect, hang_up, hang_up_with, next, send, start_ws, start_ws_on};
use ex_support::{advancing, Served};
use serde_json::json;
use ulo::{Dep, Signal};
use ulo_conformance_support::ScriptedTimer;

async fn journal(server: &Served) -> Dep<Journal> {
    server.handle.get().await.expect("the journal is a singleton")
}

#[tokio::test]
async fn each_message_is_answered_under_its_id_with_the_name_its_guard_stamped() {
    let server = start_ws(builder()).await;
    let mut ada = connect(server.addr, "/chat", &[("x-name", "ada")]).await;
    let mut bob = connect(server.addr, "/chat", &[("x-name", "bob")]).await;

    send(&mut ada, json!({ "event": "say", "id": 1, "data": "hello" })).await;
    send(&mut ada, json!({ "event": "say", "id": 2, "data": "again" })).await;
    send(&mut bob, json!({ "event": "say", "id": 7, "data": "hi" })).await;

    let mut replies = vec![next(&mut ada).await, next(&mut ada).await];
    replies.sort_by_key(|reply| reply["id"].as_u64());
    assert_eq!(replies, vec![json!({ "id": 1, "data": "ada: hello" }), json!({ "id": 2, "data": "ada: again" })]);
    assert_eq!(next(&mut bob).await, json!({ "id": 7, "data": "bob: hi" }));

    hang_up(ada).await;
    hang_up(bob).await;
    server.stop().await.unwrap();
}

#[tokio::test]
async fn every_connect_is_paired_with_a_disconnect_that_still_reads_the_session() {
    let server = start_ws(builder()).await;
    let journal = journal(&server).await;
    let mut ada = connect(server.addr, "/chat", &[("x-name", "ada")]).await;
    let mut bob = connect(server.addr, "/chat", &[("x-name", "bob")]).await;
    for (socket, id) in [(&mut ada, 1), (&mut bob, 2)] {
        send(socket, json!({ "event": "say", "id": id, "data": "ready" })).await;
        next(socket).await;
    }
    hang_up(ada).await;
    hang_up_with(bob, 4001, "bye").await;
    server.stop().await.unwrap();

    let entries = journal.entries();
    let of = |name: &str| entries.iter().filter(|entry| entry.contains(name)).cloned().collect::<Vec<_>>();
    assert_eq!(of("ada"), ["connected ada", "disconnected ada: ClientClose { code: 1000, reason: \"\" }"]);
    assert_eq!(of("bob"), ["connected bob", "disconnected bob: ClientClose { code: 4001, reason: \"bye\" }"]);
    assert_eq!(entries.len(), 4);
}

#[tokio::test]
async fn the_drain_closes_a_connection_with_1001_and_the_shutdown_phrase_and_its_disconnect_runs() {
    let clock = ScriptedTimer::new();
    let server = start_ws_on(builder(), clock.clone()).await;
    let journal = journal(&server).await;
    let mut ada = connect(server.addr, "/chat", &[("x-name", "ada")]).await;
    send(&mut ada, json!({ "event": "say", "id": 1, "data": "ready" })).await;
    next(&mut ada).await;

    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    assert_eq!(close_frame(&mut ada).await, Some((1001, "the server is shutting down".to_owned())));
    hang_up(ada).await;

    let report = advancing(&clock, DRAIN, closing).await.unwrap().unwrap();
    assert_eq!(report.signal.name(), "deploy");
    assert_eq!(clock.elapsed(), Duration::ZERO, "an idle connection ends the drain before its window");
    assert_eq!(journal.entries(), ["connected ada", "disconnected ada: Drain"]);
}
