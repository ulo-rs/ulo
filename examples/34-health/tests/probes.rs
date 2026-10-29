use std::time::Duration;

use ex_34_health::{Cache, LAG, builder};
use ex_support::{get, listen, start_on};
use serde_json::{Value, json};
use ulo::{Dep, Signal};
use ulo_conformance_support::ScriptedTimer;
use ulo_health::Readiness;
use ulo_runtime_tokio::Tokio;

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

/// Nothing answers a probe before `listen`, so the state before it is read in process; after it,
/// the same answer arrives over HTTP, naming the check that holds it down.
#[tokio::test]
async fn ready_is_503_naming_the_cold_cache_until_it_warms_and_live_never_waits_on_it() {
    let connected = builder().runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();
    let readiness = connected.get::<Readiness>().await.unwrap();
    let before = readiness.readiness().await;
    assert!(!before.is_up());
    assert_eq!(before.results().iter().map(|r| r.name()).collect::<Vec<_>>(), ["cache"]);

    let server = listen(connected).await;
    let ready = get(server.addr, "/health/ready").await;
    assert_eq!((ready.status, ready.header("content-type")), (503, Some("application/health+json")));
    assert_eq!(body(&ready.body), json!({ "status": "fail", "checks": { "cache": [{ "status": "fail", "output": "the cache is cold" }] } }));
    let live = get(server.addr, "/health/live").await;
    assert_eq!((live.status, body(&live.body)), (200, json!({ "status": "pass", "checks": {} })));

    let cache: Dep<Cache> = server.handle.get().await.unwrap();
    cache.warm_up();
    let ready = get(server.addr, "/health/ready").await;
    assert_eq!(ready.status, 200);
    assert_eq!(body(&ready.body), json!({ "status": "pass", "checks": { "cache": [{ "status": "pass" }] } }));
    server.stop().await.unwrap();
}

/// Readiness turns false when the close begins, `LAG` before the app stops accepting: probes are
/// answered 503 naming `lifecycle` while the listener is still open, then the drain starts.
#[tokio::test]
async fn ready_turns_503_at_the_start_of_the_close_and_the_listener_closes_a_lag_later() {
    let clock = ScriptedTimer::new();
    let server = start_on(builder(), clock.clone()).await;
    let cache: Dep<Cache> = server.handle.get().await.unwrap();
    cache.warm_up();
    assert_eq!(get(server.addr, "/health/ready").await.status, 200);

    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    let begun = tokio::time::Instant::now();
    while clock.live(LAG) == 0 {
        assert!(begun.elapsed() < Duration::from_secs(10), "the health module never began its lag");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let ready = get(server.addr, "/health/ready").await;
    assert_eq!(ready.status, 503);
    assert_eq!(body(&ready.body), json!({ "status": "fail", "checks": { "lifecycle": [{ "status": "fail", "output": "the app is stopping" }] } }));
    assert_eq!(get(server.addr, "/health/live").await.status, 200, "liveness does not follow readiness");
    assert!(tokio::time::timeout(Duration::from_millis(50), server.handle.draining()).await.is_err(), "the drain began before the lag passed");

    clock.advance(LAG);
    server.handle.draining().await;
    let late = std::net::TcpStream::connect(server.addr).map(drop).map_err(|e| e.kind());
    assert_eq!(late, Err(std::io::ErrorKind::ConnectionRefused));
    let report = closing.await.unwrap().unwrap();
    assert_eq!((report.signal.name(), report.abandoned), ("deploy", 0));
    assert_eq!(clock.elapsed(), LAG);
}
