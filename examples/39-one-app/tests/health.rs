mod common;

use ex_39_one_app::LAG;
use ex_support::{advancing, get};
use serde_json::{Value, json};
use ulo::testing::ScriptedTimer;

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

#[tokio::test]
async fn ready_is_503_naming_the_closed_store_until_it_opens_and_live_never_waits_on_it() {
    let clock = ScriptedTimer::new();
    let app = common::start(Some(clock.clone())).await;

    let ready = get(app.http, "/health/ready").await;
    assert_eq!((ready.status, ready.header("content-type")), (503, Some("application/health+json")));
    assert_eq!(body(&ready.body), json!({ "status": "fail", "checks": { "store": [{ "status": "fail", "output": "the store is closed" }] } }));
    let live = get(app.http, "/health/live").await;
    assert_eq!((live.status, body(&live.body)), (200, json!({ "status": "pass", "checks": {} })));

    app.store().await.open();
    let ready = get(app.http, "/health/ready").await;
    assert_eq!(ready.status, 200);
    assert_eq!(body(&ready.body), json!({ "status": "pass", "checks": { "store": [{ "status": "pass" }] } }));
    advancing(&clock, LAG, app.served.stop()).await.expect("closes");
}
