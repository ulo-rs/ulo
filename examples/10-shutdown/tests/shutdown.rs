use std::sync::Arc;

use ex_10_shutdown::dying::{Doomed, Dying};
use ex_10_shutdown::{DRAIN, Gate, builder};
use ex_support::{GUARD, Reply, advancing, get, start_on};
use tokio::task::JoinHandle;
use tokio::sync::Notify;
use ulo::{Dep, Phase, ServeEnd, ShutdownFailure, Signal, TypeName};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::Tokio;
use ulo::testing::ScriptedTimer;

fn request(addr: std::net::SocketAddr, path: &'static str) -> JoinHandle<Reply> {
    tokio::spawn(get(addr, path))
}

#[tokio::test]
async fn an_in_flight_request_finishes_inside_the_drain_and_a_late_connection_is_refused() {
    let clock = ScriptedTimer::new();
    let server = start_on(builder(), clock.clone()).await;
    let gate: Dep<Gate> = server.handle.get().await.unwrap();

    let work = request(server.addr, "/work");
    let watch = request(server.addr, "/watch");
    gate.entered.acquire_many(2).await.unwrap().forget();

    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    server.handle.draining().await;

    // The app has stopped accepting: a new connection is refused, and the handler watching for the
    // drain has ended.
    let late = std::net::TcpStream::connect(server.addr).map(drop).map_err(|e| e.kind());
    assert_eq!(late, Err(std::io::ErrorKind::ConnectionRefused));
    let watched = tokio::time::timeout(std::time::Duration::from_secs(10), watch).await.expect("/watch ended with the drain").unwrap();
    assert_eq!((watched.status, watched.body.as_str()), (200, "stopped early"));

    // The held request is still inside the window, so releasing it finishes it.
    assert_eq!(clock.elapsed(), std::time::Duration::ZERO);
    gate.release.add_permits(1);
    let worked = work.await.unwrap();
    assert_eq!((worked.status, worked.body.as_str()), (200, "finished"));

    let report = advancing(&clock, DRAIN, closing).await.unwrap().unwrap();
    assert_eq!(report.signal.name(), "deploy");
    assert_eq!((report.abandoned, report.terminal_skipped), (0, 0));
    assert_eq!(clock.elapsed(), std::time::Duration::ZERO, "the drain ended with its last request, not at the window");
}

#[tokio::test]
async fn a_request_still_held_when_the_window_ends_is_refused_with_the_shutdown_phrase_and_counted() {
    let clock = ScriptedTimer::new();
    let server = start_on(builder(), clock.clone()).await;
    let gate: Dep<Gate> = server.handle.get().await.unwrap();

    let work = request(server.addr, "/work");
    gate.entered.acquire().await.unwrap().forget();
    let closing = tokio::spawn(server.handle.close(Signal::new("deploy")));
    server.handle.draining().await;

    let report = advancing(&clock, DRAIN, closing).await.unwrap().unwrap();
    assert_eq!(report.signal.name(), "deploy");
    assert_eq!((report.abandoned, report.terminal_skipped), (1, 0));
    assert_eq!(clock.elapsed(), DRAIN);

    let refused = work.await.unwrap();
    assert_eq!(refused.status, 503);
    assert!(refused.body.contains("the server is shutting down"), "{}", refused.body);
}

#[tokio::test]
async fn a_server_whose_serve_loop_dies_starts_the_shutdown_and_the_report_names_it() {
    let clock = ScriptedTimer::new();
    let die = Arc::new(Notify::new());
    let app = builder()
        .runtime(Tokio::current())
        .timer(clock.clone())
        .wire()
        .unwrap()
        .connect()
        .await
        .unwrap()
        .bind(Server::new("127.0.0.1:0"))
        .bind(Dying { die: Arc::clone(&die) })
        .listen()
        .await
        .unwrap();
    let addr = app.addresses().first().unwrap().addr;
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    let gate: Dep<Gate> = handle.get().await.unwrap();

    let work = request(addr, "/work");
    gate.entered.acquire().await.unwrap().forget();
    assert_eq!(handle.phase(), Phase::Running);

    die.notify_one();
    tokio::time::timeout(GUARD, handle.draining()).await.expect("the dead server started the drain");

    // The healthy server drains: the held request is still inside the window and finishes.
    gate.release.add_permits(1);
    let worked = work.await.unwrap();
    assert_eq!((worked.status, worked.body.as_str()), (200, "finished"));

    let error = advancing(&clock, DRAIN, serving).await.unwrap().expect_err("a dead server is a failed shutdown");
    assert_eq!(handle.phase(), Phase::Closed);
    assert_eq!((error.report.abandoned, error.report.terminal_skipped), (0, 0));
    let [ShutdownFailure::Serve { transport, end: ServeEnd::Errored(reason) }] = &*error.failures else {
        panic!("expected the one `Serve` failure: {error}");
    };
    assert_eq!(*transport, TypeName::of::<Doomed>());
    assert_eq!(reason.to_string(), "the listener was lost");
}
