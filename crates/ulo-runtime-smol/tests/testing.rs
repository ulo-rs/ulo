//! `ulo_runtime_smol::testing::start` serves a `TestApp` on the smol runtime whose `block_on` the
//! test is inside. The server binds no socket, so the test names no HTTP crate.

use std::sync::Arc;
use std::task::{Context, Waker};
use std::time::Duration;

use ulo::testing::{ScriptedTimer, TestApp, TestServer};
use ulo::{BoundAddr, BoxError, DrainToken, Mounted, Server, Transport, module};
use ulo_runtime_smol::testing::start;
use ulo_runtime_smol::{Executor, Smol};

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

fn smol() -> Smol {
    Smol::new(Arc::new(Executor::new()))
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

#[test]
fn start_serves_on_the_smol_runtime_running_the_test() {
    smol().block_on(async {
        let served = start(TestApp::of(AppModule), Beacon).await.expect("starts");
        assert_eq!(served.addr(), "127.0.0.1:4242".parse().unwrap());
        let shutdown = served.close().await.expect("close is clean");
        assert_eq!(shutdown.signal.name(), "test");
    });
}

#[test]
fn start_replaces_a_clock_set_before_it() {
    smol().block_on(async {
        let clock = ScriptedTimer::new();
        let served = start(TestApp::of(AppModule).timer(clock.clone()), Beacon).await.expect("starts");
        assert!(!sleeps_on(&served, &clock), "the app times on smol's clock, not the scripted one");
        served.close().await.expect("close is clean");
    });
}

#[test]
fn start_on_keeps_a_clock_set_after_the_runtime() {
    let smol = smol();
    smol.block_on(async {
        let clock = ScriptedTimer::new();
        let app = TestApp::of(AppModule).runtime(smol.clone()).timer(clock.clone());
        let served = TestServer::start_on(app, Beacon).await.expect("starts");
        assert!(sleeps_on(&served, &clock), "the app times on the scripted clock");
        served.close().await.expect("close is clean");
    });
}

#[test]
#[should_panic(expected = "inside `Smol::block_on`")]
fn start_outside_block_on_panics() {
    let outside = async { start(TestApp::of(AppModule), Beacon).await.map(drop) };
    let _ = async_io::block_on(outside);
}
