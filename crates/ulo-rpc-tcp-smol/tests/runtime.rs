//! The runtime a smol TCP link's tasks run on: a client's link given none refuses, naming
//! `.with_runtime(..)`, before any I/O: in `connect`, where `RpcClient::new` builds the client, and
//! in a client module's init hook, failing the app's `connect`. A server's link given none takes
//! its app's runtime in `prepare`, and one given a runtime keeps it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ulo::{
    App, BoxFuture, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Signal, Spawn, StartupError, TaskHandle, Timer,
    injectable, routes,
};
use ulo_rpc::{Link, Payload, RpcClient, RpcClientModule};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;
use ulo_smol::Smol;
use ulo_transport::{Classify, ErrorKind};

#[derive(Debug)]
struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("never raised")
    }
}

impl std::error::Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[injectable]
struct Adder;

#[routes]
impl Adder {
    #[ulo_rpc::message("runtime.add")]
    async fn add(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0 + 1)
    }
}

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Adder>();
    }
}

/// Imports one `RpcClientModule` over the link it is given.
struct ClientRoot(Mutex<Option<Tcp>>);

impl Module for ClientRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(link) = self.0.lock().expect("the root's lock is not poisoned").take() {
            m.import(RpcClientModule::for_root(link));
        }
    }
}

/// A runtime that counts what it is asked to spawn, spawning it on smol.
#[derive(Clone)]
struct Counting {
    inner: Smol,
    spawned: Arc<AtomicUsize>,
}

impl Counting {
    fn new() -> Counting {
        Counting { inner: OnSmol::runtime(), spawned: Arc::new(AtomicUsize::new(0)) }
    }

    fn spawned(&self) -> usize {
        self.spawned.load(Ordering::SeqCst)
    }
}

impl Timer for Counting {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.inner.sleep(d)
    }

    fn now(&self) -> Instant {
        self.inner.now()
    }
}

impl Spawn for Counting {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.inner.spawn(fut)
    }
}

fn refusal_names_with_runtime(refused: &dyn std::fmt::Display) {
    assert!(refused.to_string().contains("`.with_runtime(..)`"), "the refusal names `.with_runtime(..)`: {refused}");
}

#[test]
fn a_client_link_given_no_runtime_refuses_to_connect() {
    match futures_executor::block_on(Tcp::new("127.0.0.1:7000").connect()) {
        Ok(_) => panic!("a link with no runtime connected"),
        Err(refused) => refusal_names_with_runtime(&refused),
    }
}

#[test]
fn a_client_on_a_link_given_no_runtime_is_refused_where_it_is_built() {
    OnSmol::block_on(async {
        match RpcClient::new(Tcp::new("127.0.0.1:7000"), Arc::new(OnSmol::runtime())) {
            Ok(_) => panic!("a client was built on a link with no runtime"),
            Err(refused) => refusal_names_with_runtime(&refused),
        }
    });
}

#[test]
fn a_client_module_on_a_link_given_no_runtime_fails_connect() {
    OnSmol::block_on(async {
        let wired = App::builder(ClientRoot(Mutex::new(Some(Tcp::new("127.0.0.1:7000")))))
            .runtime(OnSmol::runtime())
            .wire()
            .expect("the client wires");
        match wired.connect().await {
            Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(refused), .. })) => {
                refusal_names_with_runtime(&refused);
            }
            Err(other) => panic!("expected the client module's init hook to fail `connect`, got: {other}"),
            Ok(app) => {
                let _ = app.close(Signal::new("runtime")).await;
                panic!("a client module over a link with no runtime connected");
            }
        }
    });
}

/// Serves `link` on an app whose runtime is `app_runtime`, answers one call from a client given
/// its own runtime, and stops.
async fn serve_one_call(link: Tcp, app_runtime: Counting) {
    let server = App::builder(ServerRoot)
        .runtime(app_runtime)
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(link))
        .listen()
        .await
        .expect("the server's link listens");
    let addr = server.addresses()[0].addr;
    let handle = server.handle();
    let serving = ulo::spawn_with(&OnSmol::runtime(), server.serve(std::future::pending::<Signal>()));
    let client = RpcClient::new(Tcp::new(addr).with_runtime(OnSmol::runtime()), Arc::new(OnSmol::runtime())).expect("the client's link is usable");
    assert_eq!(client.request::<_, u32>("runtime.add", &41u32).await.expect("the call is answered"), 42);
    drop(client);
    let _ = handle.close(Signal::new("runtime")).await;
    let _ = serving.await;
}

#[test]
fn a_server_link_given_no_runtime_runs_its_tasks_on_its_app_s() {
    OnSmol::block_on(async {
        let app = Counting::new();
        serve_one_call(Tcp::new("127.0.0.1:0"), app.clone()).await;
        // The accept loop, the forwarder, the connection, and its writer, beside the server's own.
        assert!(app.spawned() >= 4, "the app's runtime was handed {} tasks", app.spawned());
    });
}

#[test]
fn a_server_link_given_a_runtime_runs_its_tasks_there() {
    OnSmol::block_on(async {
        let (link, app) = (Counting::new(), Counting::new());
        serve_one_call(Tcp::new("127.0.0.1:0").with_runtime(link.clone()), app).await;
        // The accept loop, the forwarder, the connection, and its writer.
        assert_eq!(link.spawned(), 4, "the link's runtime was handed {} tasks", link.spawned());
    });
}
