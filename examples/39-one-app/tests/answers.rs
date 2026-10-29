mod common;

use ex_39_one_app::grpc::pb::Note;
use ex_39_one_app::LAG;
use ex_support::advancing;
use ex_support::rpc::bounded;
use ex_support::ws::{connect, hang_up, next, send};
use ex_support::{get, send as raw};
use serde_json::json;
use ulo_config::Env;
use ulo_conformance_support::ScriptedTimer;

#[tokio::test]
async fn every_transport_answers_and_writes_to_the_one_store_in_the_order_they_were_bound() {
    let clock = ScriptedTimer::new();
    let app = common::start(Some(clock.clone())).await;
    assert_eq!(app.order, ["http", "ws", "rpc", "grpc"]);
    app.store().await.open();

    let posted = raw(app.http, "POST /notes/anvil HTTP/1.1\r\nhost: example\r\nconnection: close\r\ncontent-length: 0\r\n\r\n").await;
    assert_eq!((posted.status, posted.body.as_str()), (200, "1"));

    let mut socket = connect(app.ws, "/notes", &[]).await;
    send(&mut socket, json!({ "event": "add", "id": 7, "data": "bellows" })).await;
    assert_eq!(next(&mut socket).await, json!({ "id": 7, "data": 2 }));
    hang_up(socket).await;

    let counted = bounded(app.rpc_client().request::<_, u32>("notes.add", "crucible")).await;
    assert_eq!(counted.expect("answered"), 3);

    let counted = app.grpc_client().await.add(Note { text: "die".into() }).await.expect("Add answers").into_inner();
    assert_eq!(counted.n, 4);

    let notes = get(app.http, "/notes").await;
    assert_eq!(notes.status, 200);
    assert_eq!(serde_json::from_str::<Vec<String>>(&notes.body).unwrap(), ["http: anvil", "ws: bellows", "rpc: crucible", "grpc: die"]);
    assert_eq!(get(app.http, "/greeting").await.body, "hello from the defaults");
    advancing(&clock, LAG, app.served.stop()).await.expect("closes");
}

#[tokio::test]
async fn the_environment_reaches_the_handlers_through_the_configuration() {
    let clock = ScriptedTimer::new();
    let app = common::start_in(Some(clock.clone()), Env::from_map([("APP_GREETING", "hello from the shell")])).await;
    assert_eq!(get(app.http, "/greeting").await.body, "hello from the shell");
    advancing(&clock, LAG, app.served.stop()).await.expect("closes");
}
