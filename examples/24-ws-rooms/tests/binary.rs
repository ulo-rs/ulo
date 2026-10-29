use ex_support::Bin;
use ex_support::ws::{connect, hang_up, next, send};
use serde_json::json;

#[tokio::test]
async fn the_binary_serves_rooms_on_the_port_it_is_given_and_closes_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-24-ws-rooms"), &[]);
    let mut socket = connect(bin.addr(), "/rooms", &[]).await;
    send(&mut socket, json!({ "event": "ping", "id": 1 })).await;
    assert_eq!(next(&mut socket).await, json!({ "id": 1, "data": "pong" }));
    hang_up(socket).await;
    let exit = tokio::task::spawn_blocking(move || bin.terminate()).await.unwrap();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout.last().map(String::as_str), Some("closed on SIGTERM"));
    assert_eq!(exit.stderr, "");
}
