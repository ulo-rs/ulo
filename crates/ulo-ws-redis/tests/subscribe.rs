//! A server's `listen()` over the Redis adapter waits for Redis to confirm the subscription, and
//! fails as `Bind` when none comes within `connect_within`: with the last connection error as the
//! cause when Redis refuses connections, and saying Redis did not answer when it accepts and stays
//! silent. A zero bound is refused by the adapter's `prepare`, which fails `connect`.
//!
//! The bound runs on the app's `Timer`, which these tests script: no bound elapses in real time,
//! and a clock that refuses, or an adapter runtime built without a time driver, is a configuration
//! the tests set up directly. No Redis runs: a closed port, a socket that accepts and says
//! nothing, and a stand-in that confirms.

use std::error::Error as _;
use std::net::TcpListener as StdListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ulo::{
    App, BoxFuture, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Permanent, RUNTIME_SHUT_DOWN, Refused, RuntimeShutDown, StartupError,
    Timer, injectable, is_permanent, routes,
};
use ulo_conformance_support::ScriptedTimer;
use ulo_runtime_tokio::Tokio;
use ulo_ws::WsModule;
use ulo_ws_redis::Redis;

// `dropped.rs` reads the open count; these tests read the accepted count only.
#[allow(dead_code)]
mod standin;
use standin::Standin;

/// The adapter's bound in these tests; the scripted clock moves past it, the wall clock does not.
const WITHIN: Duration = Duration::from_secs(5);

/// The pause between a lost subscription's connection attempts.
const RECONNECT_DELAY: Duration = Duration::from_secs(1);

/// How long the tests wait, in real time, for the adapter to reach a state they then act on.
const PATIENCE: Duration = Duration::from_secs(10);

#[injectable]
struct Members;

#[routes]
#[ulo_ws::gateway(path = "/members", port = own)]
impl Members {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// `WsModule` over the adapter it is given, and the one gateway.
struct Root(std::sync::Mutex<Option<Redis>>);

impl Root {
    fn over(adapter: Redis) -> Root {
        Root(std::sync::Mutex::new(Some(adapter)))
    }
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(adapter) = self.0.lock().expect("the root's lock is not poisoned").take() {
            m.import(WsModule::for_root().broadcast(adapter));
        }
        m.controller::<Members>();
    }
}

/// `listen()` over `adapter` on an app timed by `clock`.
#[allow(clippy::result_large_err, reason = "`StartupError` is the error `listen` returns, handed through whole")]
async fn listen_on(adapter: Redis, clock: impl Timer) -> Result<(), StartupError> {
    App::builder(Root::over(adapter))
        .runtime(Tokio::current())
        .timer(clock)
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .map(drop)
}

/// Waits, in real time, until `done` holds, failing naming `what` after [`PATIENCE`].
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// A port that refuses connections for as long as the test runs.
fn closed_port() -> String {
    let held = StdListener::bind("127.0.0.1:0").expect("a local port");
    format!("redis://{}", held.local_addr().expect("its address"))
}

fn is_refusal(error: &(dyn std::error::Error + 'static)) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("refused") || text.contains("os error")
}

/// The error a failed `Bind` carries, unredacted: `Redacted` reports no `source()` of its own.
fn bind_cause(error: StartupError) -> ulo::BoxError {
    match error {
        StartupError::Bind { source, .. } => source.into_inner(),
        other => panic!("expected `Bind`, got: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_over_a_refusing_redis_fails_as_bind_naming_the_connection_error() {
    let clock = ScriptedTimer::new();
    let listening = tokio::spawn(listen_on(Redis::url(closed_port()).connect_within(WITHIN), clock.clone()));
    // The bound is armed on the app's clock at its full length, and the first attempt has failed
    // once the reconnect delay is armed.
    until("the bound and a failed first attempt", || clock.live(WITHIN) == 1 && clock.live(RECONNECT_DELAY) == 1).await;
    assert!(!listening.is_finished(), "listen returned before the app's clock reached the bound");
    clock.advance(WITHIN);
    let error = listening.await.expect("listen did not panic").expect_err("listen succeeded with no Redis");
    let StartupError::Bind { transport, source } = &error else { panic!("expected `Bind`, got: {error}") };
    assert!(transport.to_string().contains("Ws"), "the failed transport is the WebSocket server: {transport}");
    let text = source.to_string();
    assert!(text.contains("did not confirm the broadcast subscription"), "the cause names the subscription: {text}");
    assert!(text.contains(&format!("{WITHIN:?}")), "the cause names the bound: {text}");
    assert!(is_refusal(source), "the cause carries the last connection error, not only the timeout: {text}");
    assert!(!is_permanent(&error), "an unreachable Redis is retryable");
    let cause = bind_cause(error);
    assert!(cause.source().is_some_and(is_refusal), "the last connection error is the cause's `source()`: {cause:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_over_a_silent_redis_fails_as_bind_saying_it_did_not_answer() {
    let silent = StdListener::bind("127.0.0.1:0").expect("a local port");
    silent.set_nonblocking(true).expect("non-blocking");
    let url = format!("redis://{}", silent.local_addr().expect("its address"));
    let silent = tokio::net::TcpListener::from_std(silent).expect("the listener on the runtime");
    let accepted = Arc::new(AtomicUsize::new(0));
    let counting = Arc::clone(&accepted);
    // Accepts and holds every connection, answering none.
    let held = tokio::spawn(async move {
        let mut open = Vec::new();
        while let Ok((stream, _)) = silent.accept().await {
            counting.fetch_add(1, Ordering::SeqCst);
            open.push(stream);
        }
    });
    let clock = ScriptedTimer::new();
    let listening = tokio::spawn(listen_on(Redis::url(url).connect_within(WITHIN), clock.clone()));
    until("the bound and the held connection", || clock.live(WITHIN) == 1 && accepted.load(Ordering::SeqCst) == 1).await;
    clock.advance(WITHIN);
    let outcome = listening.await.expect("listen did not panic");
    held.abort();
    let error = outcome.expect_err("listen succeeded against a Redis that never answered");
    let StartupError::Bind { source, .. } = &error else { panic!("expected `Bind`, got: {error}") };
    let text = source.to_string();
    assert!(text.contains("did not confirm the broadcast subscription"), "{text}");
    assert!(text.contains("Redis did not answer"), "a hang has no connection error, and says so: {text}");
    assert!(!is_permanent(&error), "a silent Redis is retryable");
    let cause = bind_cause(error);
    assert!(cause.source().is_none(), "no attempt failed, so the cause has no source: {cause:?}");
}

/// An adapter runtime with IO and no time driver.
fn runtime_without_time() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_io().build().expect("a runtime with IO and no time driver")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adapter_runtime_with_no_time_driver_listens() {
    let (standin, url) = Standin::start().await;
    let runtime = runtime_without_time();
    let adapter = Redis::url(url).with_handle(runtime.handle().clone());
    // The app's runtime times the bound; the adapter's has no clock to time it with.
    let listened = listen_on(adapter, Tokio::current().timer()).await;
    runtime.shutdown_background();
    listened.unwrap_or_else(|error| panic!("listen failed over an adapter runtime with no time driver: {error}"));
    assert!(standin.accepted() >= 1, "the subscription never reached Redis");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adapter_runtime_with_no_time_driver_still_reports_a_refusing_redis() {
    let clock = ScriptedTimer::new();
    let runtime = runtime_without_time();
    let adapter = Redis::url(closed_port()).with_handle(runtime.handle().clone()).connect_within(WITHIN);
    let listening = tokio::spawn(listen_on(adapter, clock.clone()));
    // The reconnect delay is the app's, so it is armed though the adapter's runtime has no clock.
    until("the bound and a failed first attempt", || clock.live(WITHIN) == 1 && clock.live(RECONNECT_DELAY) == 1).await;
    clock.advance(WITHIN);
    let outcome = listening.await.expect("listen did not panic");
    runtime.shutdown_background();
    let error = outcome.expect_err("listen succeeded with no Redis");
    let StartupError::Bind { source, .. } = &error else { panic!("expected `Bind`, got: {error}") };
    assert!(source.to_string().contains("did not confirm the broadcast subscription"), "{source}");
    let cause = bind_cause(error);
    assert!(cause.source().is_some_and(is_refusal), "the last connection error is the cause's source: {cause:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clock_that_refuses_the_bound_fails_listen_as_a_permanent_runtime_shut_down() {
    let clock = ScriptedTimer::new();
    clock.refuse(WITHIN);
    let outcome = listen_on(Redis::url(closed_port()).connect_within(WITHIN), clock.clone()).await;
    let error = outcome.expect_err("listen succeeded on a clock that refuses its bound");
    let StartupError::Bind { source, .. } = &error else { panic!("expected `Bind`, got: {error}") };
    assert!(is_permanent(&error), "a clock that has shut down refuses every later start too: {error}");
    let ended = source
        .downcast_ref::<Permanent>()
        .and_then(|permanent| permanent.source())
        .and_then(|inner| inner.downcast_ref::<RuntimeShutDown>())
        .unwrap_or_else(|| panic!("the failure is not a `Permanent` `RuntimeShutDown`: {source:?}"));
    assert_eq!(ended.reason(), RUNTIME_SHUT_DOWN);
    assert!(ended.source().is_some_and(|cause| cause.is::<Refused>()), "the refusal is the source: {ended:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clock_that_refuses_the_reconnect_delay_ends_the_subscription_as_a_permanent_runtime_shut_down() {
    let clock = ScriptedTimer::new();
    clock.refuse(RECONNECT_DELAY);
    // Bounded in real time: a task that ignored the refusal would retry without pausing.
    let listening = tokio::time::timeout(PATIENCE, listen_on(Redis::url(closed_port()).connect_within(WITHIN), clock.clone()));
    let outcome = listening.await.expect("listen did not end on the refused reconnect delay");
    let error = outcome.expect_err("listen succeeded with no Redis");
    assert!(is_permanent(&error), "a subscription that cannot pace itself cannot be retried: {error}");
    let cause = bind_cause(error);
    let ended = cause
        .downcast_ref::<Permanent>()
        .and_then(|permanent| permanent.source())
        .and_then(|inner| inner.downcast_ref::<RuntimeShutDown>())
        .unwrap_or_else(|| panic!("the failure is not a `Permanent` `RuntimeShutDown`: {cause:?}"));
    assert!(ended.source().is_some_and(is_refusal), "the last connection error is the source: {ended:?}");
    assert_eq!(clock.refused(RECONNECT_DELAY), 1, "the task asked for the delay once and ended on the refusal");
}

/// The scripted clock, refusing a sleep of the bound's length only after moving past it: the
/// refusal arrives once the bound has passed.
struct RefusesAfterTheBound(ScriptedTimer);

impl Timer for RefusesAfterTheBound {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.0.sleep(d)
    }

    fn now(&self) -> Instant {
        self.0.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        if d != WITHIN {
            return self.0.try_sleep(d);
        }
        let clock = self.0.clone();
        Box::pin(async move {
            clock.advance(d);
            Err(Refused)
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bound_that_had_passed_when_the_clock_refused_reports_its_expiry() {
    let clock = RefusesAfterTheBound(ScriptedTimer::new());
    let outcome = listen_on(Redis::url(closed_port()).connect_within(WITHIN), clock).await;
    let error = outcome.expect_err("listen succeeded with no Redis");
    let StartupError::Bind { source, .. } = &error else { panic!("expected `Bind`, got: {error}") };
    assert!(source.to_string().contains("did not confirm the broadcast subscription"), "the bound's expiry is reported: {source}");
    assert!(source.downcast_ref::<Permanent>().is_none(), "a passed bound is not reported as a shut-down clock: {source:?}");
    assert!(!is_permanent(&error), "an expiry is retryable");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_bound_fails_connect() {
    let wired = App::builder(Root::over(Redis::url("redis://127.0.0.1:1").connect_within(Duration::ZERO)))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires");
    match wired.connect().await {
        Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(error), .. })) => {
            assert!(error.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {error}");
        }
        Err(other) => panic!("expected `WsModule`'s init hook to fail `connect`, got: {other}"),
        Ok(_) => panic!("an app connected over an adapter whose bound is zero"),
    }
}

/// `WsModule` over an adapter held on `handle`'s runtime, and the one gateway.
struct OnRuntime(tokio::runtime::Handle);

impl Module for OnRuntime {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Redis::url("redis://127.0.0.1:1").with_handle(self.0.clone()).connect_within(WITHIN)));
        m.controller::<Members>();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_adapter_on_a_runtime_without_io_fails_connect_naming_enable_io() {
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let wired = App::builder(OnRuntime(runtime.handle().clone())).runtime(Tokio::current()).wire().expect("the app wires");
    let connected = wired.connect().await;
    runtime.shutdown_background();
    match connected {
        Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(error), .. })) => {
            assert!(error.to_string().contains("enable_io"), "the refusal names `enable_io`: {error}");
        }
        Err(other) => panic!("expected `WsModule`'s init hook to fail `connect`, got: {other}"),
        Ok(_) => panic!("an app connected over an adapter whose runtime has no IO driver"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_over_an_adapter_whose_runtime_has_shut_down_fails_as_a_permanent_bind() {
    let handle = std::thread::spawn(|| {
        tokio::runtime::Builder::new_current_thread().enable_all().build().expect("a runtime to shut down").handle().clone()
    })
    .join()
    .expect("the runtime shuts down");
    let listened = App::builder(OnRuntime(handle))
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await;
    let error = listened.map(drop).expect_err("listen succeeded over an adapter whose runtime has shut down");
    let StartupError::Bind { .. } = &error else { panic!("expected `Bind`, got: {error}") };
    assert!(is_permanent(&error), "a runtime that has shut down refuses every later start too: {error}");
}
