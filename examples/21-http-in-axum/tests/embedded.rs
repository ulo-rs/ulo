use std::future::pending;
use std::net::SocketAddr;
use std::time::Duration;

use ex_21_http_in_axum::{Gate, builder, embedded, router};
use ex_support::{advancing, get};
use tokio::task::JoinHandle;
use ulo::{AppHandle, Dep, Shutdown, Signal};
use ulo::testing::ScriptedTimer;
use ulo_runtime_tokio::Tokio;

const DRAIN: Duration = Duration::from_secs(10);

struct Hosted {
    addr: SocketAddr,
    handle: AppHandle,
    running: JoinHandle<Result<Shutdown, ulo::BoxError>>,
}

async fn host(clock: &ScriptedTimer) -> Hosted {
    let server = embedded();
    let embedded = server.handle();
    let app = builder()
        .runtime(Tokio::current())
        .timer(clock.clone())
        .drain_timeout(DRAIN)
        .wire()
        .unwrap()
        .connect()
        .await
        .unwrap()
        .bind(server)
        .listen()
        .await
        .unwrap();
    let handle = app.handle();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let serve = axum::serve(listener, router(&embedded));
    let running = tokio::spawn(async move { ulo_http_axum::run(app, &embedded, serve, pending::<Signal>()).await });
    Hosted { addr, handle, running }
}

#[tokio::test]
async fn an_axum_route_and_a_ulo_route_answer_side_by_side_on_one_socket() {
    let clock = ScriptedTimer::new();
    let hosted = host(&clock).await;

    let own = get(hosted.addr, "/health").await;
    assert_eq!((own.status, own.body.as_str()), (200, "axum is up"));
    let ulo = get(hosted.addr, "/api/hello").await;
    assert_eq!((ulo.status, ulo.body.as_str()), (200, "hello from ulo"));
    let miss = get(hosted.addr, "/api/nowhere").await;
    assert_eq!((miss.status, miss.header("content-type")), (404, Some("application/problem+json")), "a miss under the mount is ulo's 404");
    let outside = get(hosted.addr, "/hello").await;
    assert_eq!((outside.status, outside.header("content-type")), (404, None), "outside /api the miss is axum's, with no body");

    let closed = hosted.handle.close(Signal::new("test")).await.expect("close is clean");
    assert_eq!(closed.signal.name(), "test");
    hosted.running.await.unwrap().expect("the host ends with the app");
}

#[tokio::test]
async fn a_ulo_request_still_running_when_the_drain_window_ends_is_a_503_with_the_shutdown_phrase() {
    let clock = ScriptedTimer::new();
    let hosted = host(&clock).await;
    let gate: Dep<Gate> = hosted.handle.get().await.unwrap();

    let work = tokio::spawn(get(hosted.addr, "/api/work"));
    gate.entered.acquire().await.unwrap().forget();
    let closing = tokio::spawn(hosted.handle.close(Signal::new("deploy")));
    hosted.handle.draining().await;

    let report = advancing(&clock, DRAIN, closing).await.unwrap().unwrap();
    assert_eq!((report.signal.name(), report.abandoned), ("deploy", 1));
    let refused = work.await.unwrap();
    assert_eq!(refused.status, 503);
    assert!(refused.body.contains("the server is shutting down"), "{}", refused.body);
    hosted.running.await.unwrap().expect("the host ends with the app");
}
