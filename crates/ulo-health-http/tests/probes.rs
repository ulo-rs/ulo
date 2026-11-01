//! The probe routes `HealthHttp` mounts, called in process: paths, the 200 or 503 each probe
//! answers by readiness state, the `application/health+json` body, the `checks` object that stays
//! off until asked for, redaction of a down reason, and readiness turning false at `Stopping`
//! before the app refuses work. Integrations DESIGN §3.2-§3.3; capability "health surfaces" (G17).

mod support;

use std::time::{Duration, Instant};

use http::{Method, StatusCode};
use serde_json::{Value, json};
use ulo_health::HealthModule;
use ulo_health_http::{HealthHttp, MEDIA_TYPE};

use support::{PATIENCE, Reply, Root, Toggle, start};

fn assert_probe(reply: &Reply, status: StatusCode, handler: &str, body: Value) {
    assert_eq!(reply.status, status);
    assert_eq!(reply.handler(), Some(handler), "the route that answered");
    assert_eq!(reply.header("content-type"), Some(MEDIA_TYPE));
    assert_eq!(reply.json(), body);
}

#[tokio::test]
async fn each_probe_answers_200_pass_as_health_json_from_its_own_route() {
    let app = start(Root::new()).await;
    assert_eq!(MEDIA_TYPE, "application/health+json");
    for (path, handler) in [("/health/live", "live"), ("/health/ready", "ready"), ("/health/started", "started")] {
        let reply = app.get(path).await;
        assert_probe(&reply, StatusCode::OK, handler, json!({ "status": "pass" }));
    }
    app.stop().await;
}

#[tokio::test]
async fn a_down_readiness_check_answers_503_fail_on_ready_and_leaves_live_and_started_up() {
    let db = Toggle::readiness("db");
    let app = start(Root::new().check(&db)).await;
    assert_probe(&app.get("/health/ready").await, StatusCode::OK, "ready", json!({ "status": "pass" }));

    db.set(false);
    assert_probe(&app.get("/health/ready").await, StatusCode::SERVICE_UNAVAILABLE, "ready", json!({ "status": "fail" }));
    assert_probe(&app.get("/health/live").await, StatusCode::OK, "live", json!({ "status": "pass" }));
    assert_probe(&app.get("/health/started").await, StatusCode::OK, "started", json!({ "status": "pass" }));

    db.set(true);
    assert_probe(&app.get("/health/ready").await, StatusCode::OK, "ready", json!({ "status": "pass" }));
    app.stop().await;
}

#[tokio::test]
async fn a_down_liveness_check_answers_503_on_live_alone() {
    let own = Toggle::liveness("event-loop");
    let app = start(Root::new().check(&own)).await;
    own.set(false);
    assert_probe(&app.get("/health/live").await, StatusCode::SERVICE_UNAVAILABLE, "live", json!({ "status": "fail" }));
    assert_probe(&app.get("/health/ready").await, StatusCode::OK, "ready", json!({ "status": "pass" }));
    assert_probe(&app.get("/health/started").await, StatusCode::OK, "started", json!({ "status": "pass" }));
    app.stop().await;
}

#[tokio::test]
async fn the_checks_object_is_off_by_default_and_lists_every_check_when_asked_for() {
    let down = Toggle::down_with("db", "connection refused");
    let up = Toggle::readiness("cache");
    let twin_a = Toggle::readiness("queue");
    let twin_b = Toggle::down_with("queue", "lagging");

    let plain = start(Root::new().check(&down).check(&up)).await;
    let reply = plain.get("/health/ready").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.json().as_object().map(|body| body.keys().cloned().collect::<Vec<_>>()), Some(vec!["status".to_owned()]));
    plain.stop().await;

    let mut root = Root::new().check(&down).check(&up).check(&twin_a).check(&twin_b);
    root.http = HealthHttp::new().detail(true);
    let detailed = start(root).await;
    let reply = detailed.get("/health/ready").await;
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    let body = reply.json();
    assert_eq!(body["status"], "fail");
    let checks = body["checks"].as_object().unwrap_or_else(|| panic!("no checks object in {body}"));
    assert_eq!(checks.len(), 3, "one entry per check name");
    assert_eq!(checks["db"], json!([{ "status": "fail", "output": "connection refused" }]));
    assert_eq!(checks["cache"], json!([{ "status": "pass" }]));
    let twins = checks["queue"].as_array().unwrap_or_else(|| panic!("queue is not a list in {body}"));
    assert_eq!(twins.len(), 2, "two checks of one name are both listed");
    assert!(twins.contains(&json!({ "status": "pass" })) && twins.contains(&json!({ "status": "fail", "output": "lagging" })), "{twins:?}");

    let live = detailed.get("/health/live").await;
    assert_eq!(live.json(), json!({ "status": "pass", "checks": {} }), "detail on a probe with no check of its own");
    detailed.stop().await;
}

#[tokio::test]
async fn a_down_reason_is_redacted_before_it_reaches_the_body() {
    const REASON: &str = "refused for password hunter2-s3cret at host";
    let leaky = Toggle::down_with("db", REASON);
    let mut root = Root::new().check(&leaky);
    root.http = HealthHttp::new().detail(true);
    root.secret = Some("hunter2-s3cret");
    let app = start(root).await;
    let reply = app.get("/health/ready").await;
    let text = String::from_utf8_lossy(&reply.body).into_owned();
    assert!(!text.contains("hunter2-s3cret"), "the secret reached the body: {text}");
    let body = reply.json();
    assert_eq!(body["checks"]["db"][0]["status"], "fail");
    let redacted = app.handle.redact(ulo::BoxError::from(REASON)).to_string();
    assert_ne!(redacted, REASON, "the app's redaction left the reason as it was");
    assert_eq!(body["checks"]["db"][0]["output"], redacted.as_str(), "the reason with the secret replaced: {body}");
    app.stop().await;
}

#[tokio::test]
async fn configured_paths_are_the_paths_served_with_slashes_normalised_and_the_defaults_gone() {
    let mut root = Root::new();
    root.http = HealthHttp::new().live_path("alive/").ready_path("/up").started_path("/boot/status");
    let app = start(root).await;
    assert_probe(&app.get("/alive").await, StatusCode::OK, "live", json!({ "status": "pass" }));
    assert_probe(&app.get("/up").await, StatusCode::OK, "ready", json!({ "status": "pass" }));
    assert_probe(&app.get("/boot/status").await, StatusCode::OK, "started", json!({ "status": "pass" }));
    for default in ["/health/live", "/health/ready", "/health/started"] {
        let reply = app.get(default).await;
        assert_eq!(reply.status, StatusCode::NOT_FOUND, "{default}");
        assert_eq!(reply.handler(), None, "{default}");
    }
    app.stop().await;
}

#[tokio::test]
async fn a_probe_answers_get_and_head_alone() {
    let app = start(Root::new()).await;
    let head = app.send(Method::HEAD, "/health/ready").await;
    assert_eq!(head.status, StatusCode::OK);
    assert_eq!(head.handler(), Some("ready"));
    assert!(head.body.is_empty(), "a HEAD answer carries no body");
    let post = app.send(Method::POST, "/health/ready").await;
    assert_eq!(post.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(post.handler(), None);
    app.stop().await;
}

#[tokio::test]
async fn a_check_that_never_answers_is_down_at_its_bound_and_readiness_answers_503() {
    let stuck = Toggle::stuck("slow", Duration::from_millis(150));
    let mut root = Root::new().check(&stuck);
    root.http = HealthHttp::new().detail(true);
    let app = start(root).await;
    let started = Instant::now();
    let reply = app.get("/health/ready").await;
    let took = started.elapsed();
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.json()["checks"]["slow"][0]["status"], "fail");
    assert!(took >= Duration::from_millis(140), "answered after {took:?}, before the 150 ms bound");
    assert!(took < PATIENCE, "answered after {took:?}");
    app.stop().await;
}

#[tokio::test]
async fn readiness_turns_503_at_stopping_during_the_lag_while_live_stays_200() {
    const LAG: Duration = Duration::from_millis(1500);
    let mut root = Root::new();
    root.health = HealthModule::new().lag(LAG);
    root.http = HealthHttp::new().detail(true);
    let app = start(root).await;
    assert_probe(&app.get("/health/ready").await, StatusCode::OK, "ready", json!({ "status": "pass", "checks": {} }));

    let handle = app.handle.clone();
    let closing = tokio::spawn(async move { handle.close(ulo::Signal::new("test")).await });
    let begun = Instant::now();
    let ready = loop {
        let reply = app.get("/health/ready").await;
        if reply.status == StatusCode::SERVICE_UNAVAILABLE {
            break reply;
        }
        assert!(begun.elapsed() < PATIENCE, "readiness never turned 503");
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    assert!(begun.elapsed() < LAG, "readiness turned false only after the lag ({:?})", begun.elapsed());
    assert!(!closing.is_finished(), "the app had finished closing before it answered 503 once");
    let body = ready.json();
    assert_eq!(body["status"], "fail");
    assert_eq!(body["checks"]["lifecycle"][0]["status"], "fail", "the stopping result's name: {body}");
    assert_probe(&app.get("/health/live").await, StatusCode::OK, "live", json!({ "status": "pass", "checks": {} }));
    assert_eq!(app.get("/health/started").await.status, StatusCode::OK);

    let _ = tokio::time::timeout(PATIENCE, closing).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, app.serving_end()).await;
}
