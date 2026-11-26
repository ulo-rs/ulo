//! `TestServer` serves a `TestApp` on port 0, answers a request, and releases its port whether the
//! test closes it, closes the app through its handle first, or drops it.

use std::net::SocketAddr;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use ulo::testing::{TestApp, TestServer};
use ulo::{Signal, controller, handlers, module};
use ulo_http_hyper::{Server, ServerOn};
use ulo_listener_smol::SmolListener;
use ulo_runtime_tokio::testing::start;

#[controller]
struct Notes;

#[handlers]
impl Notes {
    #[ulo_http::get("/notes/1")]
    fn first(&self) -> &'static str {
        "note one"
    }
}

#[module(controllers = [Notes])]
struct AppModule;

async fn get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream
        .write_all(format!("GET {path} HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n").as_bytes())
        .await
        .expect("write");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.expect("read");
    raw
}

/// Polls until a connection to `addr` is refused: an aborted task releases its listener when the
/// runtime next polls it, not within `drop`.
async fn released(addr: SocketAddr) {
    for _ in 0..500 {
        if TcpStream::connect(addr).await.is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{addr} still accepts connections");
}

#[tokio::test]
async fn it_serves_a_request_and_closes_cleanly() {
    let served = start(TestApp::of(AppModule), Server::new("127.0.0.1:0")).await.expect("starts");
    let addr = served.addr();
    assert_ne!(addr.port(), 0);
    assert_eq!(served.url("/notes/1"), format!("http://{addr}/notes/1"));
    assert_eq!(served.url("notes/1"), served.url("/notes/1"));

    let reply = get(addr, "/notes/1").await;
    assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
    assert!(reply.ends_with("note one"), "{reply}");

    let shutdown = served.close().await.expect("close is clean");
    assert_eq!(shutdown.signal.name(), "test");
    assert_eq!((shutdown.abandoned, shutdown.terminal_skipped), (0, 0));
    released(addr).await;
}

#[tokio::test]
async fn closing_the_app_first_leaves_close_answering() {
    let served = start(TestApp::of(AppModule), Server::new("127.0.0.1:0")).await.expect("starts");
    let addr = served.addr();
    let first = served.handle().close(Signal::new("early")).await.expect("the handle closes");
    assert_eq!(first.signal.name(), "early");

    let second = served.close().await.expect("a second close joins the first");
    assert_eq!(second.signal.name(), "early");
    released(addr).await;
}

#[tokio::test]
async fn dropping_without_close_releases_the_port() {
    let served = start(TestApp::of(AppModule), Server::new("127.0.0.1:0")).await.expect("starts");
    let addr = served.addr();
    assert!(get(addr, "/notes/1").await.starts_with("HTTP/1.1 200"));
    drop(served);
    released(addr).await;
}

#[test]
fn it_serves_on_smol_through_start_on() {
    let smol = ulo_runtime_smol::Smol::new(std::sync::Arc::new(ulo_runtime_smol::Executor::new()));
    smol.block_on(async {
        let served = TestServer::start_on(
            TestApp::of(AppModule).runtime(smol.clone()),
            ServerOn::<SmolListener>::new("127.0.0.1:0"),
        )
        .await
        .expect("starts");
        let addr = served.addr();
        let mut stream = async_net::TcpStream::connect(addr).await.expect("connect");
        futures::AsyncWriteExt::write_all(&mut stream, b"GET /notes/1 HTTP/1.1\r\nhost: t\r\nconnection: close\r\n\r\n")
            .await
            .expect("write");
        let mut raw = String::new();
        futures::AsyncReadExt::read_to_string(&mut stream, &mut raw).await.expect("read");
        assert!(raw.starts_with("HTTP/1.1 200") && raw.ends_with("note one"), "{raw}");

        served.close().await.expect("close is clean");
        assert!(async_net::TcpStream::connect(addr).await.is_err());
    });
}
