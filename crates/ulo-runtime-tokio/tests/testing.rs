//! `ulo_runtime_tokio::testing::start` serves a `TestApp` on the current tokio runtime. The server
//! binds no socket, so the test names no HTTP crate.

use std::task::{Context, Waker};
use std::time::Duration;

use ulo::testing::{ScriptedTimer, TestApp, TestServer};
use ulo::{BoundAddr, BoxError, DrainToken, Mounted, Server, Transport, module};
use ulo_runtime_tokio::Tokio;
use ulo_runtime_tokio::testing::start;

struct Wire;

impl Transport for Wire {
    const KEY: &'static str = "wire";
    type Cx = ();
    type Reply = ();
}

#[module]
struct AppModule;

struct Beacon;

impl Server for Beacon {
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

    fn bound(&self) -> Vec<BoundAddr> {
        vec![BoundAddr::new("wire", "127.0.0.1:4242".parse().expect("a socket address"))]
    }
}

/// Whether a 30 s sleep on the app's clock is registered on `clock`, polled once.
fn sleeps_on(served: &TestServer, clock: &ScriptedTimer) -> bool {
    let grace = Duration::from_secs(30);
    let mut sleep = served.handle().timer().expect("a bound app has a clock").sleep(grace);
    assert!(sleep.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending());
    let registered = clock.live(grace) == 1;
    drop(sleep);
    registered
}

#[tokio::test]
async fn start_serves_on_the_current_tokio_runtime() {
    let served = start(TestApp::of(AppModule), Beacon).await.expect("starts");
    assert_eq!(served.addr(), "127.0.0.1:4242".parse().unwrap());
    let shutdown = served.close().await.expect("close is clean");
    assert_eq!(shutdown.signal.name(), "test");
}

#[tokio::test]
async fn start_keeps_a_clock_set_before_it() {
    let clock = ScriptedTimer::new();
    let served = start(TestApp::of(AppModule).timer(clock.clone()), Beacon).await.expect("starts");
    assert!(sleeps_on(&served, &clock), "the app times on the scripted clock, not tokio's");
    served.close().await.expect("close is clean");
}

#[tokio::test]
async fn start_on_keeps_a_clock_set_after_the_runtime() {
    let clock = ScriptedTimer::new();
    let app = TestApp::of(AppModule).runtime(Tokio::current()).timer(clock.clone());
    let served = TestServer::start_on(app, Beacon).await.expect("starts");
    assert!(sleeps_on(&served, &clock), "the app times on the scripted clock");
    served.close().await.expect("close is clean");
}
