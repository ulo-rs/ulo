mod common;

use std::time::Duration;

use common::Running;
use ex_39_one_app::grpc::pb::Note;
use ex_39_one_app::{DRAIN, LAG};
use ex_support::rpc::{bounded, eventually};
use ex_support::ws::{close_frame, connect, hang_up, next, send};
use ex_support::{Reply, advancing, get};
use serde_json::{Value, json};
use tokio::task::JoinHandle;
use ulo::Signal;
use ulo_rpc::ErrorKind;
use ulo::testing::ScriptedTimer;

struct Held {
    http: JoinHandle<Reply>,
    ws: ex_support::ws::Socket,
    rpc: JoinHandle<Result<String, ulo_rpc::RpcError>>,
    grpc: JoinHandle<Result<tonic::Response<Note>, tonic::Status>>,
}

/// One `hold` in flight on each transport, all four inside their handlers.
async fn hold_on_every_transport(app: &Running) -> Held {
    let http = tokio::spawn(get(app.http, "/hold"));
    let mut ws = connect(app.ws, "/notes", &[]).await;
    send(&mut ws, json!({ "event": "hold", "id": 1, "data": null })).await;
    let rpc_client = app.rpc_client();
    let rpc = tokio::spawn(async move { rpc_client.request::<_, String>("notes.hold", &()).timeout(CALL_DEADLINE).await });
    let mut grpc_client = app.grpc_client().await;
    let grpc = tokio::spawn(async move { grpc_client.hold(Note { text: "grpc".into() }).await });
    bounded(app.gate().await.entered.acquire_many(4)).await.unwrap().forget();
    Held { http, ws, rpc, grpc }
}

/// Starts the close and runs it past the lag: the listeners close and the drain begins.
async fn close_into_drain(app: &Running, clock: &ScriptedTimer) -> JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>> {
    let closing = tokio::spawn(app.served.handle.close(Signal::new("deploy")));
    let begun = tokio::time::Instant::now();
    while clock.live(LAG) == 0 {
        assert!(begun.elapsed() < Duration::from_secs(10), "the health module never began its lag");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    closing
}

fn body(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("`{text}`: {e}"))
}

#[tokio::test]
async fn one_close_drains_all_four_transports_and_an_execution_released_inside_the_window_finishes() {
    let clock = ScriptedTimer::new();
    let app = common::start(Some(clock.clone())).await;
    app.store().await.open();
    assert_eq!(get(app.http, "/health/ready").await.status, 200);
    let mut held = hold_on_every_transport(&app).await;

    let closing = close_into_drain(&app, &clock).await;
    let ready = get(app.http, "/health/ready").await;
    assert_eq!(ready.status, 503, "readiness turned false when the close began");
    assert_eq!(body(&ready.body), json!({ "status": "fail", "checks": { "lifecycle": [{ "status": "fail", "output": "the app is stopping" }] } }));
    clock.advance(LAG);
    app.served.handle.draining().await;

    // `draining()` is the notice: the core fires it, then hands each server its drain, and each
    // server drops its listener on its own task. On a multi-thread runtime the notice can be seen first.
    for (name, addr) in [("http", app.http), ("ws", app.ws), ("rpc", app.rpc), ("grpc", app.grpc)] {
        let refused = || std::net::TcpStream::connect(addr).is_err_and(|e| e.kind() == std::io::ErrorKind::ConnectionRefused);
        eventually(&format!("{name} {addr} refusing connections"), refused).await;
    }

    app.gate().await.release.add_permits(4);
    let http = bounded(held.http).await.unwrap();
    assert_eq!((http.status, http.body.as_str()), (200, "held"));
    assert_eq!(next(&mut held.ws).await, json!({ "id": 1, "data": "held" }));
    assert_eq!(close_frame(&mut held.ws).await, Some((1001, "the server is shutting down".to_owned())));
    hang_up(held.ws).await;
    assert_eq!(bounded(held.rpc).await.unwrap().expect("answered"), "held");
    assert_eq!(bounded(held.grpc).await.unwrap().expect("answered").into_inner().text, "grpc");

    let report = advancing(&clock, DRAIN, closing).await.unwrap().unwrap();
    assert_eq!((report.signal.name(), report.abandoned, report.terminal_skipped), ("deploy", 0, 0));
    assert_eq!(clock.elapsed(), LAG, "the drain ended with its last execution, not at the window");
}

#[tokio::test]
async fn an_execution_still_held_when_the_window_ends_is_refused_with_the_shutdown_phrase_on_every_transport() {
    let clock = ScriptedTimer::new();
    let app = common::start(Some(clock.clone())).await;
    app.store().await.open();
    let mut held = hold_on_every_transport(&app).await;

    let closing = close_into_drain(&app, &clock).await;
    clock.advance(LAG);
    app.served.handle.draining().await;
    let report = common::advancing_any(&clock, &[DRAIN, AFTER_DRAIN], closing).await.unwrap().unwrap();
    assert_eq!((report.signal.name(), report.abandoned, report.terminal_skipped), ("deploy", 4, 0), "one abandoned execution per transport");
    assert_eq!(clock.elapsed(), LAG + DRAIN + AFTER_DRAIN);

    let http = bounded(held.http).await.unwrap();
    assert_eq!(http.status, 503);
    assert!(http.body.contains("the server is shutting down"), "{}", http.body);
    assert_eq!(close_frame(&mut held.ws).await, Some((1001, "the server is shutting down".to_owned())));
    let rpc = bounded(held.rpc).await.unwrap().expect_err("refused");
    assert_eq!((rpc.kind(), rpc.message()), (ErrorKind::Unavailable, "the server is shutting down"));
    let grpc = bounded(held.grpc).await.unwrap().expect_err("refused");
    assert_eq!((grpc.code(), grpc.message()), (tonic::Code::Unavailable, "the server is shutting down"));
}

/// Longer than the whole scripted close. The callee times a call's deadline on the app's clock, so
/// a call made with the client's default 5 s passes its deadline at 5 s, inside the window that
/// ends at `LAG + DRAIN`, and ends `Timeout` instead of being abandoned.
const CALL_DEADLINE: Duration = Duration::from_secs(60);

/// The grace the HTTP, WebSocket and gRPC servers each give their connections in `close`, after the
/// drain window, before the app reports.
const AFTER_DRAIN: Duration = Duration::from_millis(500);
