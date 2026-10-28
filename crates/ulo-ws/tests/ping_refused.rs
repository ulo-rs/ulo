//! A connection's pings are a ticker on the app's runtime. A runtime that has shut down refuses
//! the next tick, and the pings stop there: on `sleep`, which ends at once on such a runtime, the
//! connection would ping without pause. The connection itself serves on.

mod support;

use std::time::{Duration, Instant};

use serde_json::json;
use ulo::{BoxFuture, Module, ModuleDef, ModuleIdentity, Spawn, TaskHandle, Timer, injectable, routes};
use ulo_conformance_support::ScriptedTimer;
use ulo_runtime_tokio::Tokio;
use ulo_ws::WsModule;

use support::{Running, hang_up, next_json, send_json, within};

/// The gateway's ping interval, the one length the runtime refuses.
const PING: Duration = Duration::from_secs(7);

/// The ticks after which a ticker still asking is taken to spin.
const TURNS: usize = 3;

/// A scripted clock with tokio's spawning: the app's runtime.
#[derive(Clone)]
struct Handed {
    clock: ScriptedTimer,
    spawn: Tokio,
}

impl Timer for Handed {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        Timer::try_sleep(&self.clock, d)
    }
}

impl Spawn for Handed {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawn.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        Spawn::try_spawn(&self.spawn, fut)
    }
}

#[injectable]
struct Ticking;

#[routes]
#[ulo_ws::gateway(path = "/ticking", ping_interval = ulo::Bound::After(PING), pong_timeout = ulo::Bound::Unbounded)]
impl Ticking {
    #[ulo_ws::message("echo")]
    fn echo(&self) -> &'static str {
        "echoed"
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Ticking>();
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_ping_tick_stops_the_pings_and_the_connection_serves_on() {
    let clock = ScriptedTimer::new();
    clock.refuse(PING);
    let app = Running::start_on(Root, ulo_http_hyper::Server::new("127.0.0.1:0"), Handed { clock: clock.clone(), spawn: Tokio::current() }).await;
    let mut socket = app.connect("/ticking", &[]).await;
    within("the connection's first ping tick", async {
        while clock.refused(PING) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    send_json(&mut socket, &json!({ "event": "echo", "id": 1 })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "echoed" }), "the connection stopped serving with its pings");
    let began = Instant::now();
    while clock.refused(PING) <= TURNS && began.elapsed() < Duration::from_millis(200) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(clock.refused(PING), 1, "the connection's ping ticks on a runtime refusing them: it pinged on rather than stopping");
    hang_up(socket).await;
    app.stop().await;
}
