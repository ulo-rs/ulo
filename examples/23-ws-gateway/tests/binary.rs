use ex_support::Bin;
use ex_support::ws::{connect, hang_up, next, send};
use serde_json::json;

#[tokio::test]
async fn the_binary_serves_the_gateway_on_the_port_it_is_given_and_closes_on_sigterm() {
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-23-ws-gateway"), &[]);
    let mut socket = connect(bin.addr(), "/chat", &[("x-name", "ada")]).await;
    send(&mut socket, json!({ "event": "say", "id": 1, "data": "hi" })).await;
    assert_eq!(next(&mut socket).await, json!({ "id": 1, "data": "ada: hi" }));
    hang_up(socket).await;
    let exit = tokio::task::spawn_blocking(move || bin.terminate()).await.unwrap();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stdout.last().map(String::as_str), Some("closed on SIGTERM"));
    assert_eq!(exit.stderr, "");
}
