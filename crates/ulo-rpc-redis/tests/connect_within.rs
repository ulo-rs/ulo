//! `connect_within` bounds a server's `listen` and a client's connect on the clock that times
//! each: the app's `Timer` for a server, the runtime its client was handed for a client. The
//! server here is a socket that accepts the link's connections and never answers, against which
//! the redis client's own bound ends the connect after about 16 seconds (F642). Each test scripts
//! the clock: it waits until the link has armed a wait of exactly the bound, moves the clock past
//! it, and the connect fails at once with `ConnectTimedOut` naming the bound, which the redis
//! client's own expiry never answers.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::net::TcpListener;
use ulo::testing::ScriptedTimer;
use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Refused, Runtime, Spawn, TaskHandle, Timer};
use ulo_rpc::{ConnectTimedOut, Link, Pattern};
use ulo_rpc_redis::Redis;
use ulo_runtime_tokio::Tokio;

/// The bound under test, a length the scripted clock alone times.
const WITHIN: Duration = Duration::from_secs(3);

/// How long, in real time, a test waits for the link to reach a state it then acts on: shorter
/// than the redis client's own bound, so its expiry cannot stand in for the link's.
const PATIENCE: Duration = Duration::from_secs(10);

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// A socket that accepts every connection and holds it open, writing nothing.
async fn silent() -> (String, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the silent server binds");
    let url = format!("redis://{}", listener.local_addr().expect("the silent server has an address"));
    let held = tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            held.push(stream);
        }
    });
    (url, held)
}

/// Waits in real time until a wait of exactly `WITHIN` is live on `clock`.
async fn armed(clock: &ScriptedTimer, what: &str) {
    let began = Instant::now();
    while clock.live(WITHIN) == 0 {
        assert!(began.elapsed() < PATIENCE, "{what} armed no wait of `connect_within` within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn timed_out(outcome: Result<(), BoxError>, what: &str) {
    let refused = outcome.err().unwrap_or_else(|| panic!("{what} connected to a server that never answers"));
    let timed_out = refused.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("{what} failed other than as `ConnectTimedOut`: {refused}"));
    assert_eq!((timed_out.within(), timed_out.link()), (WITHIN, "Redis"), "{what}: {refused}");
    assert!(timed_out.address().starts_with("127.0.0.1:"), "{what} names the server by its address: {refused}");
}

/// Planted break: `listen` connecting with no bound of its own, which ends at the redis client's
/// expiry, `timed out`, after about 16 seconds.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_s_listen_ends_at_connect_within_on_the_app_s_clock() {
    let (url, _held) = silent().await;
    let clock = ScriptedTimer::new();
    let app = App::builder(Root).runtime(Tokio::current()).timer(clock.clone()).wire().expect("the app wires").connect().await.expect("the app connects");
    let mut link = Redis::url(url).connect_within(WITHIN);
    link.prepare(&app.handle()).await.expect("the link prepares");
    let listening = tokio::spawn(async move { link.listen(&[Pattern::from("connect_within.add").into()]).await.map(drop) });
    armed(&clock, "`listen`").await;
    clock.advance(WITHIN);
    let outcome = tokio::time::timeout(PATIENCE, listening).await.expect("`listen` ended at its bound").expect("the listen task ran");
    timed_out(outcome, "`listen`");
}

/// The test's tokio runtime with a scripted clock, as a client is handed it.
struct Clocked {
    tokio: Tokio,
    clock: ScriptedTimer,
}

impl Timer for Clocked {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        self.clock.try_sleep(d)
    }
}

impl Spawn for Clocked {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.tokio.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        self.tokio.try_spawn(fut)
    }
}

/// Planted break: a client's connect with no bound of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_s_connect_ends_at_connect_within_on_its_runtime_s_clock() {
    let (url, _held) = silent().await;
    let clock = ScriptedTimer::new();
    let runtime: Arc<dyn Runtime> = Arc::new(Clocked { tokio: Tokio::current(), clock: clock.clone() });
    let link = Redis::url(url).connect_within(WITHIN);
    let connecting = tokio::spawn(async move { link.connect(&runtime).await.map(drop) });
    armed(&clock, "the client's connect").await;
    clock.advance(WITHIN);
    let outcome = tokio::time::timeout(PATIENCE, connecting).await.expect("the connect ended at its bound").expect("the connect task ran");
    timed_out(outcome, "the client's connect");
}

/// A zero bound is refused where a client takes the link and in a server's `prepare`, before
/// any I/O. Planted break: `usable` not reading `connect_within`.
#[tokio::test(flavor = "current_thread")]
async fn a_zero_connect_within_is_refused_before_any_io() {
    let zero = Redis::url("redis://127.0.0.1:1").connect_within(Duration::ZERO);
    let refused = zero.usable().expect_err("`usable` accepted a zero `connect_within`");
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let mut zero = zero;
    let refused = zero.prepare(&app.handle()).await.expect_err("`prepare` accepted a zero `connect_within`");
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");
}
