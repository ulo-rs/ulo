//! `HealthGrpc` over a served `grpc.health.v1`: `""` follows readiness, one `Watch` message per
//! change and none for an unchanged evaluation, `Bound::Unbounded` leaving `""` to lifecycle
//! transitions, `NOT_SERVING` written at `Stopping` before the drain, and a zero interval refused
//! at `wire()`. Integrations DESIGN §3.4; capability "health surfaces" (G17).

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use tonic::Streaming;
use tonic::transport::{Channel, Endpoint};
use tonic_health::pb::health_check_response::ServingStatus as Wire;
use tonic_health::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::{HealthCheckRequest, HealthCheckResponse};
use ulo::{
    App, AppHandle, Bound, BoxFuture, Dep, FailureReason, HookKind, Module, ModuleDef, ModuleIdentity, ShutdownFailure, Signal, StartupError, WiringError,
};
use ulo_health::{Check, Health, HealthModule};
use ulo_grpc::GrpcHealth;
use ulo_health_grpc::{HealthGrpc, ZeroInterval};

const PATIENCE: Duration = Duration::from_secs(10);

async fn within<F: std::future::Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(PATIENCE, fut).await {
        Ok(output) => output,
        Err(_) => panic!("{what} did not happen within {PATIENCE:?}"),
    }
}

#[derive(Debug)]
struct Down;

impl fmt::Display for Down {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("down")
    }
}

impl Error for Down {}

/// A readiness check a test flips, counting its evaluations.
struct Flag {
    up: AtomicBool,
    runs: AtomicUsize,
}

struct Contributed(Arc<Flag>);

impl Check for Contributed {
    fn name(&self) -> &str {
        "flag"
    }

    fn check(&self) -> BoxFuture<'_, Health> {
        self.0.runs.fetch_add(1, Ordering::SeqCst);
        let up = self.0.up.load(Ordering::SeqCst);
        Box::pin(async move { if up { Health::Up } else { Health::down(Down) } })
    }
}

struct Root {
    flag: Arc<Flag>,
    interval: Bound,
    health: Arc<Mutex<Option<GrpcHealth>>>,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(HealthModule::new());
        m.import(HealthGrpc::new().interval(self.interval));
        let check: Arc<dyn Check> = Arc::new(Contributed(Arc::clone(&self.flag)));
        m.contribute::<dyn Check>().value(check);
        let slot = Arc::clone(&self.health);
        m.on_init(move |health: Dep<GrpcHealth>| {
            let slot = Arc::clone(&slot);
            async move {
                *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some((*health).clone());
                Ok::<(), std::convert::Infallible>(())
            }
        });
    }
}

struct Running {
    flag: Arc<Flag>,
    /// The reporter the server and the bridge share, captured by an init hook of the root.
    health: Arc<Mutex<Option<GrpcHealth>>>,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    channel: Channel,
}

async fn start(interval: Bound) -> Running {
    start_capped(interval, None).await
}

/// `cap` is the app's `shutdown_timeout`: a zero cap is spent at the trigger, so no shutdown hook
/// starts.
async fn start_capped(interval: Bound, cap: Option<Duration>) -> Running {
    let flag = Arc::new(Flag { up: AtomicBool::new(true), runs: AtomicUsize::new(0) });
    let health = Arc::new(Mutex::new(None));
    let mut builder = App::builder(Root { flag: Arc::clone(&flag), interval, health: Arc::clone(&health) }).runtime(ulo_runtime_tokio::Tokio::current()).drain_timeout(Duration::from_secs(4));
    if let Some(cap) = cap {
        builder = builder.shutdown_timeout(cap);
    }
    let app = builder
        .wire()
        .unwrap_or_else(|error| panic!("the app wires: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app connects: {error}"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .listen();
    let app = within("listen", app).await.unwrap_or_else(|error| panic!("the app listens: {error}"));
    let addr = match app.addresses().as_slice() {
        [bound] => bound.addr,
        other => panic!("expected one bound address, got {other:?}"),
    };
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let channel = within("the channel", async {
        Endpoint::from_shared(format!("http://{addr}")).expect("an endpoint").connect().await.unwrap_or_else(|error| panic!("connect: {error}"))
    })
    .await;
    Running { flag, health, handle, serving, channel }
}

impl Running {
    fn client(&self) -> HealthClient<Channel> {
        HealthClient::new(self.channel.clone())
    }

    async fn watch(&self) -> Streaming<HealthCheckResponse> {
        let reply = self.client().watch(HealthCheckRequest { service: String::new() }).await;
        reply.unwrap_or_else(|status| panic!("Watch failed: {status:?}")).into_inner()
    }

    async fn check(&self) -> Wire {
        let reply = self.client().check(HealthCheckRequest { service: String::new() }).await;
        reply.unwrap_or_else(|status| panic!("Check failed: {status:?}")).into_inner().status()
    }

    async fn stop(self) {
        let _ = within("close", self.handle.close(Signal::new("test"))).await;
        let _ = within("serve", self.serving).await;
    }
}

async fn next(updates: &mut Streaming<HealthCheckResponse>) -> Wire {
    let update = within("the next health update", updates.next()).await;
    match update {
        Some(Ok(response)) => response.status(),
        other => panic!("expected an update, got {other:?}"),
    }
}

async fn silent_for(updates: &mut Streaming<HealthCheckResponse>, window: Duration) {
    if let Ok(update) = tokio::time::timeout(window, updates.next()).await {
        panic!("a Watch subscriber received {update:?} though nothing changed");
    }
}

#[tokio::test]
async fn the_overall_status_follows_readiness_with_one_watch_message_per_change() {
    let app = start(Bound::After(Duration::from_millis(40))).await;
    let mut updates = app.watch().await;
    assert_eq!(next(&mut updates).await, Wire::Serving);

    // Many evaluations pass with the check up; none of them reaches the subscriber.
    let before = app.flag.runs.load(Ordering::SeqCst);
    silent_for(&mut updates, Duration::from_millis(400)).await;
    assert!(app.flag.runs.load(Ordering::SeqCst) >= before + 3, "the bridge did not re-evaluate on its interval");

    app.flag.up.store(false, Ordering::SeqCst);
    assert_eq!(next(&mut updates).await, Wire::NotServing);
    assert_eq!(app.check().await, Wire::NotServing);
    silent_for(&mut updates, Duration::from_millis(300)).await;

    app.flag.up.store(true, Ordering::SeqCst);
    assert_eq!(next(&mut updates).await, Wire::Serving);
    assert_eq!(app.check().await, Wire::Serving);
    drop(updates);
    app.stop().await;
}

#[tokio::test]
async fn an_unbounded_interval_evaluates_once_and_leaves_the_status_to_the_lifecycle() {
    let app = start(Bound::Unbounded).await;
    let mut updates = app.watch().await;
    assert_eq!(next(&mut updates).await, Wire::Serving);

    app.flag.up.store(false, Ordering::SeqCst);
    silent_for(&mut updates, Duration::from_millis(400)).await;
    assert_eq!(app.check().await, Wire::Serving, "a check going down is not followed without an interval");
    assert_eq!(app.flag.runs.load(Ordering::SeqCst), 1, "the one evaluation at start");

    let stopping = tokio::spawn(app.stop());
    assert_eq!(next(&mut updates).await, Wire::NotServing, "Stopping writes NOT_SERVING");
    drop(updates);
    within("close", stopping).await.unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn stopping_writes_not_serving_before_the_drain_ends_the_watch() {
    let app = start(Bound::After(Duration::from_millis(40))).await;
    let mut updates = app.watch().await;
    assert_eq!(next(&mut updates).await, Wire::Serving);

    let stopping = tokio::spawn(app.stop());
    assert_eq!(next(&mut updates).await, Wire::NotServing);
    // The status stays NOT_SERVING: the re-evaluation task has ended and nothing writes SERVING.
    let rest = within("the watch's end", async {
        let mut seen = Vec::new();
        while let Some(update) = updates.next().await {
            seen.push(update.map(|response| response.status()).map_err(|status| status.code()));
        }
        seen
    })
    .await;
    assert!(rest.iter().all(|update| *update != Ok(Wire::Serving)), "SERVING after NOT_SERVING: {rest:?}");
    within("close", stopping).await.unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn a_spent_shutdown_cap_and_no_interval_still_turn_the_overall_status_not_serving_at_the_drain() {
    // The cap is spent at the trigger, so the bridge's before-shutdown hook is skipped; with
    // `Unbounded` there is no re-evaluation task to end at `Stopping` either. Only the server's
    // own write at the drain can turn `""`, and the check stays up throughout.
    let app = start_capped(Bound::Unbounded, Some(Duration::ZERO)).await;
    assert_eq!(app.check().await, Wire::Serving);
    let mut updates = app.watch().await;
    assert_eq!(next(&mut updates).await, Wire::Serving);
    let health = app.health.lock().unwrap_or_else(PoisonError::into_inner).clone().expect("the init hook captured the reporter");
    assert_eq!(health.status("").await, Some(ServingStatus::Serving), "the status before the close");

    // The shutdown closes the connections before a client could read the status (a spent cap
    // allows no wait), so it is read from the reporter's stored copy, `Check`'s source.
    let closed = within("close", app.handle.close(Signal::new("test"))).await;
    assert_eq!(health.status("").await, Some(ServingStatus::NotServing), "the drain's write for the delegated overall status");
    // The expired cap polls the server's `close` once; that poll still aborts the connection a
    // `Watch` holds open, which graceful shutdown alone would wait on.
    let rest = within("the watch's end", async {
        let mut seen = Vec::new();
        while let Some(update) = updates.next().await {
            seen.push(update.map(|response| response.status()).map_err(|status| status.code()));
        }
        seen
    })
    .await;
    assert!(rest.iter().all(|update| *update != Ok(Wire::Serving)), "SERVING after the drain: {rest:?}");
    let error = closed.expect_err("a spent cap reports the hook it skipped");
    let skipped: Vec<_> = error
        .failures
        .iter()
        .filter_map(|failure| match failure {
            ShutdownFailure::Hook { hook, key, reason: FailureReason::Skipped } => Some((*hook, format!("{:#}", key.key().type_name()))),
            _ => None,
        })
        .collect();
    assert!(skipped.iter().all(|(hook, _)| *hook == HookKind::BeforeApplicationShutdown), "only before-shutdown hooks start after the cap: {skipped:?}");
    let mut owners: Vec<&str> = skipped.iter().map(|(_, owner)| owner.as_str()).collect();
    owners.sort_unstable();
    assert_eq!(
        owners,
        ["ulo_health::readiness::Readiness", "ulo_health_grpc::bridge::HealthGrpc"],
        "the hooks skipped: readiness's and the bridge module's"
    );
    assert_eq!(app.flag.runs.load(Ordering::SeqCst), 1, "no evaluation after the first: the status turned without readiness");
    let _ = within("serve", app.serving).await;
}

#[tokio::test]
async fn a_zero_interval_is_refused_at_wiring_naming_zero_interval() {
    let flag = Arc::new(Flag { up: AtomicBool::new(true), runs: AtomicUsize::new(0) });
    let wired = App::builder(Root { flag, interval: Bound::After(Duration::ZERO), health: Arc::default() }).runtime(ulo_runtime_tokio::Tokio::current()).wire();
    let Err(StartupError::Wiring(errors)) = wired else {
        panic!("a zero interval wired");
    };
    let refusals: Vec<_> = errors
        .iter()
        .filter_map(|error| match error {
            WiringError::ValueFailed { error, .. } => error.downcast_ref::<ZeroInterval>(),
            _ => None,
        })
        .collect();
    assert_eq!(refusals.len(), 1, "one ZeroInterval among {errors}");
}
