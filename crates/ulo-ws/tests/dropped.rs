//! An app serving a gateway through the hand-off on the HTTP server's port, bound and dropped
//! without `serve` or `close`, lets go of its WebSocket hub once its last handle drops: the hub's
//! delivery task ends, its subscription to the broadcast adapter with it, the adapter drops, no
//! task is left on the runtime, and the app's singletons drop (F434). An HTTP server refused at
//! `listen()` releases the hand-off it prepared (F449). Two HTTP servers sharing the hand-off: the
//! first one's drop leaves the gateways to the second.

mod support;

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use serde_json::json;
use ulo::{App, BoundAddr, BoxError, BoxFuture, DrainToken, Module, ModuleDef, ModuleIdentity, Mounted, Server, Signal, StartupError, injectable, routes};
use ulo_http::Http;
use ulo_runtime_tokio::Tokio;
use ulo_transport::Count;
use ulo_ws::{BroadcastAdapter, InMemory, NodeId, Target, WsModule};

/// How long a drop has to take effect on the runtime.
const PATIENCE: Duration = Duration::from_secs(5);

/// What the test watches drop.
#[derive(Default)]
struct Flags {
    subscribed: AtomicBool,
    subscription_dropped: AtomicBool,
    adapter_dropped: AtomicBool,
    singleton_dropped: AtomicBool,
}

/// Sets one of the flags when it drops.
struct OnDrop(Arc<Flags>, fn(&Flags) -> &AtomicBool);

impl Drop for OnDrop {
    fn drop(&mut self) {
        (self.1)(&self.0).store(true, Ordering::SeqCst);
    }
}

/// The in-memory adapter, recording its own drop and its subscription's. With `stalls`, its
/// subscription is a stream that never yields nor ends, whatever drops.
struct Watched {
    inner: InMemory,
    stalls: bool,
    _dropped: OnDrop,
    flags: Arc<Flags>,
}

impl BroadcastAdapter for Watched {
    fn publish(&self, target: Target, frame: Bytes) -> BoxFuture<'static, Result<(), BoxError>> {
        self.inner.publish(target, frame)
    }

    fn subscribe(&self, node: NodeId) -> BoxStream<'static, (Target, Bytes)> {
        self.flags.subscribed.store(true, Ordering::SeqCst);
        let dropped = OnDrop(Arc::clone(&self.flags), |flags| &flags.subscription_dropped);
        let carried = if self.stalls { futures_util::stream::pending().boxed() } else { self.inner.subscribe(node) };
        Box::pin(carried.map(move |item| {
            let _ = &dropped;
            item
        }))
    }
}

#[injectable]
struct Lobby;

#[routes]
#[ulo_ws::gateway(path = "/lobby")]
impl Lobby {
    #[ulo_ws::message("ping")]
    fn ping(&self) -> &'static str {
        "pong"
    }
}

/// `WsModule` over a [`Watched`] adapter, the one gateway, and a singleton recording its drop.
struct Root(Arc<Flags>, bool);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let adapter = Watched {
            inner: InMemory::new(),
            stalls: self.1,
            _dropped: OnDrop(Arc::clone(&self.0), |flags| &flags.adapter_dropped),
            flags: Arc::clone(&self.0),
        };
        m.import(WsModule::for_root().broadcast(adapter));
        m.controller::<Lobby>();
        m.value(OnDrop(Arc::clone(&self.0), |flags| &flags.singleton_dropped));
    }
}

fn alive_tasks() -> usize {
    tokio::runtime::Handle::current().metrics().num_alive_tasks()
}

/// Waits until `done` holds, yielding to the runtime between reads, and fails naming `what`.
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Runs `fut` and fails naming `what` after [`PATIENCE`].
async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not end within {PATIENCE:?}"))
}

#[tokio::test]
async fn a_hub_whose_app_is_dropped_unclosed_ends_its_delivery_task() {
    dropped_unclosed(false).await;
}

/// The hub's drop aborts the delivery task: a subscription that never ends of itself still drops.
#[tokio::test]
async fn a_hub_dropped_unclosed_aborts_a_delivery_task_waiting_on_its_subscription() {
    dropped_unclosed(true).await;
}

async fn dropped_unclosed(stalls: bool) {
    let before = alive_tasks();
    let flags = Arc::new(Flags::default());
    let app = within(
        "the app's start",
        App::builder(Root(Arc::clone(&flags), stalls))
            .runtime(Tokio::current())
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect(),
    )
    .await
    .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
    .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
    .listen()
    .await
    .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    until("the hub's subscription", || flags.subscribed.load(Ordering::SeqCst)).await;
    let handle = app.handle();
    drop(app);
    assert!(!flags.singleton_dropped.load(Ordering::SeqCst), "the app's singletons dropped while a handle was held");
    drop(handle);
    until("the hub's subscription's drop", || flags.subscription_dropped.load(Ordering::SeqCst)).await;
    assert!(flags.adapter_dropped.load(Ordering::SeqCst), "the broadcast adapter outlived the hub's delivery task");
    assert!(flags.singleton_dropped.load(Ordering::SeqCst), "the app's singletons outlived its last handle");
    until("every task's end", || alive_tasks() == before).await;
}

/// An HTTP server refused at `listen()` by its own configuration has run the hand-off's
/// `prepare`, which built a table holding the app, and releases it as it drops: the app's
/// singletons drop once the refusal does (F449).
#[tokio::test]
async fn an_http_server_refused_at_listen_releases_the_hand_off() {
    let flags = Arc::new(Flags::default());
    let refused = within(
        "the app's start",
        App::builder(Root(Arc::clone(&flags), false))
            .runtime(Tokio::current())
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect(),
    )
    .await
    .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
    .bind(ulo_http_hyper::Server::new("127.0.0.1:0").max_inflight(Count::Max(0)))
    .listen()
    .await;
    assert!(matches!(refused, Err(StartupError::Configure(_))), "a server with `max_inflight` 0 was not refused at `listen()`");
    drop(refused);
    until("the app's singletons' drop", || flags.singleton_dropped.load(Ordering::SeqCst)).await;
}

/// An HTTP server over hyper that the test can drop while the app keeps serving: the public port,
/// taken down while the admin port serves on. `serve` waits forever, the test dropping the server
/// before any shutdown; a shutdown that still finds it closes it.
struct Droppable(Arc<Mutex<Option<ulo_http_hyper::Server>>>);

impl Droppable {
    fn take(&self) -> Option<ulo_http_hyper::Server> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }

    fn put(&self, server: ulo_http_hyper::Server) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some(server);
    }
}

impl Server for Droppable {
    type Transport = Http;

    async fn prepare(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let mut server = self.take().ok_or("the droppable server was prepared after its drop")?;
        let prepared = server.prepare(mounted).await;
        self.put(server);
        prepared
    }

    async fn bind(&mut self, mounted: Mounted<'_, Http>) -> Result<(), BoxError> {
        let mut server = self.take().ok_or("the droppable server was bound after its drop")?;
        let bound = server.bind(mounted).await;
        self.put(server);
        bound
    }

    async fn serve(&self) -> Result<(), BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: DrainToken) {}

    async fn close(&self) -> Result<(), BoxError> {
        match self.take() {
            Some(server) => Server::close(&server).await,
            None => Ok(()),
        }
    }

    fn bound(&self) -> Vec<BoundAddr> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).as_ref().map(Server::bound).unwrap_or_default()
    }
}

/// `WsModule` and the one gateway.
struct Plain;

impl Module for Plain {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Lobby>();
    }
}

/// Two HTTP servers prepare the one hand-off `WsModule` registers. The public port's server drops
/// while the app serves, and its drop releases the hand-off; the gateway on the admin port still
/// upgrades and answers a message, the table the hand-off built being held for the admin server.
#[tokio::test]
async fn a_hand_off_two_servers_share_serves_the_second_after_the_first_drops() {
    let public = Arc::new(Mutex::new(Some(ulo_http_hyper::Server::new("127.0.0.1:0"))));
    let app = within(
        "the app's start",
        App::builder(Plain)
            .runtime(Tokio::current())
            .drain_timeout(Duration::from_secs(1))
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect(),
    )
    .await
    .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
    .bind(Droppable(Arc::clone(&public)))
    .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
    .listen()
    .await
    .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let admin = match app.addresses().as_slice() {
        [_, admin] => admin.addr,
        other => panic!("expected the public and the admin address, got {other:?}"),
    };
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));

    let dropped = public.lock().unwrap_or_else(PoisonError::into_inner).take();
    assert!(dropped.is_some(), "the public server was gone before the test dropped it");
    drop(dropped);

    let upgrade = support::upgrade(admin, "/lobby", &[]).await;
    let Some(mut socket) = upgrade.socket else {
        panic!("the admin port refused the upgrade after the public server's drop: {} {:?}", upgrade.status, upgrade.body);
    };
    support::send_json(&mut socket, &json!({ "event": "ping", "id": "a" })).await;
    assert_eq!(support::next_json(&mut socket).await, json!({ "id": "a", "data": "pong" }), "the admin port's gateway did not answer");
    support::hang_up(socket).await;

    within("the app's close", handle.close(Signal::new("test"))).await.unwrap_or_else(|error| panic!("the app did not close: {error}"));
    within("`serve`'s end", serving).await.expect("the serving task panicked").unwrap_or_else(|error| panic!("`serve` failed: {error}"));
}
