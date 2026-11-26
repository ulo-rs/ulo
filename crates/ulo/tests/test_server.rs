//! `ulo::testing::TestServer` serves a `TestApp` through any `ulo::Server`, so a test of a
//! transport that is not HTTP needs no HTTP crate. The server here binds no socket: it reports a
//! fixed address and holds a flag the serving task drops with it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Waker};
use std::time::Duration;

use ulo::testing::{ScriptedTimer, TestApp, TestServer};
use ulo::{BoundAddr, BoxError, DrainToken, Mounted, Server, Signal, Transport, module};
use ulo_runtime_tokio::Tokio;

struct Wire;

impl Transport for Wire {
    const KEY: &'static str = "wire";
    type Cx = ();
    type Reply = ();
}

#[module]
struct AppModule;

/// Reports `127.0.0.1:4242`, counts its `close` and sets `serving` while its `serve` future lives.
#[derive(Default, Clone)]
struct Beacon {
    serving: Arc<AtomicBool>,
    closed: Arc<AtomicUsize>,
}

struct Serving(Arc<AtomicBool>);

impl Drop for Serving {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl Server for Beacon {
    type Transport = Wire;

    async fn bind(&mut self, _mounted: Mounted<'_, Wire>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.serving.store(true, Ordering::SeqCst);
        let _serving = Serving(self.serving.clone());
        std::future::pending().await
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        self.closed.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn bound(&self) -> Vec<BoundAddr> {
        vec![BoundAddr::new("wire", ADDR.parse().expect("a socket address"))]
    }
}

const ADDR: &str = "127.0.0.1:4242";

async fn until(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..500 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what} did not happen");
}

fn app() -> TestApp {
    TestApp::of(AppModule).runtime(Tokio::current())
}

#[tokio::test]
async fn it_serves_a_server_that_is_not_http_and_closes_it() {
    let beacon = Beacon::default();
    let served = TestServer::start_on(app(), beacon.clone()).await.expect("starts");
    let addr: SocketAddr = ADDR.parse().unwrap();
    assert_eq!(served.addr(), addr);
    until("the server serving", || beacon.serving.load(Ordering::SeqCst)).await;

    let shutdown = served.close().await.expect("close is clean");
    assert_eq!(shutdown.signal.name(), "test");
    assert_eq!((shutdown.abandoned, shutdown.terminal_skipped), (0, 0));
    assert_eq!(beacon.closed.load(Ordering::SeqCst), 1);
    assert!(!beacon.serving.load(Ordering::SeqCst));
}

#[tokio::test]
async fn url_names_the_address_and_adds_the_leading_slash() {
    let served = TestServer::start_on(app(), Beacon::default()).await.expect("starts");
    assert_eq!(served.url("/notes/1"), "http://127.0.0.1:4242/notes/1");
    assert_eq!(served.url("notes/1"), served.url("/notes/1"));
    assert_eq!(served.url(""), "http://127.0.0.1:4242/");
    served.close().await.expect("close is clean");
}

#[tokio::test]
async fn closing_the_app_first_leaves_close_answering() {
    let served = TestServer::start_on(app(), Beacon::default()).await.expect("starts");
    let first = served.handle().close(Signal::new("early")).await.expect("the handle closes");
    assert_eq!(first.signal.name(), "early");

    let second = served.close().await.expect("a second close joins the first");
    assert_eq!(second.signal.name(), "early");
}

#[tokio::test]
async fn dropping_without_close_aborts_the_serving_task() {
    let beacon = Beacon::default();
    let served = TestServer::start_on(app(), beacon.clone()).await.expect("starts");
    until("the server serving", || beacon.serving.load(Ordering::SeqCst)).await;

    drop(served);
    until("the serving task ending", || !beacon.serving.load(Ordering::SeqCst)).await;
    assert_eq!(beacon.closed.load(Ordering::SeqCst), 0, "an abort runs no shutdown");
}

#[tokio::test]
async fn a_clock_set_after_the_runtime_is_the_clock_the_app_times_with() {
    let clock = ScriptedTimer::new();
    let served = TestServer::start_on(app().timer(clock.clone()), Beacon::default()).await.expect("starts");
    let timer = served.handle().timer().expect("a bound app has a clock").clone();

    let grace = Duration::from_secs(30);
    let mut sleep = timer.sleep(grace);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(sleep.as_mut().poll(&mut cx).is_pending());
    assert_eq!(clock.live(grace), 1);
    clock.advance(grace);
    assert!(sleep.as_mut().poll(&mut cx).is_ready());
    drop(sleep);
    served.close().await.expect("close is clean");
}

/// A server that binds no address, as a broker link does.
struct Broker;

impl Server for Broker {
    type Transport = Wire;

    async fn bind(&mut self, _mounted: Mounted<'_, Wire>) -> Result<(), BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

#[tokio::test]
async fn a_server_that_binds_no_address_is_served_and_closed() {
    let served = TestServer::start_on(app(), Broker).await.expect("starts");
    assert!(served.addresses().is_empty());
    let shutdown = served.close().await.expect("close is clean");
    assert_eq!(shutdown.signal.name(), "test");
}

#[tokio::test]
async fn an_app_without_a_runtime_is_refused_at_bind() {
    let refused = TestServer::start_on(TestApp::of(AppModule), Beacon::default()).await.err().expect("refused");
    assert!(matches!(refused, ulo::StartupError::Bind { .. }), "{refused}");
}
