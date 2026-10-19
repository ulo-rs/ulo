//! An app serving a gateway through the hand-off on the HTTP server's port, bound and dropped
//! without `serve` or `close`, lets go of its WebSocket hub once its last handle drops: the hub's
//! delivery task ends, its subscription to the broadcast adapter with it, the adapter drops, no
//! task is left on the runtime, and the app's singletons drop (F434).

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::StreamExt;
use futures_util::stream::BoxStream;
use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_runtime_tokio::Tokio;
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
