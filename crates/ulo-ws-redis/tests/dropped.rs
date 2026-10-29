//! An app serving a gateway over the Redis adapter, bound, broadcasting once and dropped without
//! `serve` or `close`, lets go of both its Redis connections once its last handle drops: the
//! hub's delivery task ends, the subscription's task with it, which closes the Pub/Sub
//! connection, and the publisher's connection drops with the adapter (F434).
//!
//! The Redis server is a stand-in on a local port answering the few commands the adapter sends,
//! counting the connections it holds open: no container runs.

use std::time::{Duration, Instant};

use ulo::{App, Module, ModuleDef, ModuleIdentity, injectable, routes};
use ulo_runtime_tokio::Tokio;
use ulo_ws::{Rooms, WsModule};
use ulo_ws_redis::Redis;

mod standin;
use standin::Standin;

/// How long a drop has to reach the stand-in.
const PATIENCE: Duration = Duration::from_secs(5);

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

/// `WsModule` over the Redis adapter on `url`, and the one gateway.
struct Root(String);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root().broadcast(Redis::url(&self.0)));
        m.controller::<Members>();
    }
}

/// Waits until `done` holds, failing naming `what` after [`PATIENCE`].
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_dropped_unclosed_closes_its_redis_connections() {
    let (standin, url) = Standin::start().await;
    let app = App::builder(Root(url))
        .runtime(Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    until("the subscription's connection", || standin.open() == 1).await;
    let handle = app.handle();
    let rooms = handle.get::<Rooms>().await.unwrap_or_else(|error| panic!("`Rooms` did not resolve: {error}"));
    rooms.to_all().emit("notice", &1).await.unwrap_or_else(|error| panic!("the broadcast failed: {error}"));
    drop(rooms);
    assert_eq!((standin.accepted(), standin.open()), (2, 2), "the subscription's and the publisher's connections");
    drop(app);
    drop(handle);
    until("both connections' close", || standin.open() == 0).await;
}
