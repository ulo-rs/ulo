use ex_10_shutdown::{DRAIN, Gate, builder};
use ex_support::{Reply, advancing, get, start_on};
use tokio::task::JoinHandle;
use ulo::{Dep, Signal};
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
