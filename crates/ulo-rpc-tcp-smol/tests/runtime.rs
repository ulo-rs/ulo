//! The runtime a smol TCP link's tasks run on. Given none, a client's link runs them on the runtime
//! its client hands `connect`: the one given to `RpcClient::new` outside an app, the app's inside
//! one, so a client module needs no extra configuration. A server's link given none takes its app's
//! runtime in `prepare`. A link given a runtime with `.with_runtime(..)` keeps it on either side.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ulo::{App, BoxFuture, Module, ModuleDef, ModuleIdentity, Runtime, Signal, Spawn, TaskHandle, Timer, injectable, routes};
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

/// The tasks the link starts for one client connection: its writer and its reply reader.
const LINK_TASKS: usize = 2;
/// The client's own over that connection: the router of the reply lane.
const ROUTER: usize = 1;
/// The link's close, which a client built with `RpcClient::new` spawns once dropped; a module's
/// client is closed by the module's destroy hook instead.
const CLOSE: usize = 1;

#[test]
fn a_client_link_given_no_runtime_runs_its_tasks_on_its_client_s() {
    OnSmol::block_on(async {
        let client = Counting::new();
        let calls = client.clone();
        serve_and_call(Tcp::new("127.0.0.1:0"), Counting::new(), move |addr| async move {
            let rpc = RpcClient::new(Tcp::new(addr), Arc::new(calls)).expect("a client link given no runtime is usable");
            assert_eq!(rpc.request::<_, u32>("runtime.add", &41u32).await.expect("the call is answered"), 42);
        })
        .await;
        assert_eq!(client.spawned(), LINK_TASKS + ROUTER + CLOSE, "the client's runtime was handed {} tasks", client.spawned());
    });
}

#[test]
fn a_client_module_on_a_link_given_no_runtime_runs_its_tasks_on_its_app_s() {
    OnSmol::block_on(async {
        let client = Counting::new();
        let calls = client.clone();
        serve_and_call(Tcp::new("127.0.0.1:0"), Counting::new(), move |addr| async move {
            let app = App::builder(ClientRoot(Mutex::new(Some(Tcp::new(addr)))))
                .runtime(calls)
                .wire()
                .expect("the client wires")
                .connect()
                .await
                .expect("a client module over a link given no runtime connects");
            let rpc = (*app.get::<RpcClient>().await.expect("the client app holds an `RpcClient`")).clone();
            assert_eq!(rpc.request::<_, u32>("runtime.add", &41u32).await.expect("the call is answered"), 42);
            drop(rpc);
            let _ = app.close(Signal::new("runtime")).await;
        })
        .await;
        assert!(
            client.spawned() >= LINK_TASKS + ROUTER,
            "the client app's runtime was handed {} tasks, fewer than one connection starts",
            client.spawned()
        );
    });
}

#[test]
fn a_client_link_given_a_runtime_runs_its_tasks_there() {
    OnSmol::block_on(async {
        let (link, client) = (Counting::new(), Counting::new());
        let (on_link, calls) = (link.clone(), client.clone());
        serve_and_call(Tcp::new("127.0.0.1:0"), Counting::new(), move |addr| async move {
            let rpc = RpcClient::new(Tcp::new(addr).with_runtime(on_link), Arc::new(calls)).expect("the client's link is usable");
            assert_eq!(rpc.request::<_, u32>("runtime.add", &41u32).await.expect("the call is answered"), 42);
        })
        .await;
        assert_eq!(link.spawned(), LINK_TASKS, "the link's runtime was handed {} tasks", link.spawned());
        assert_eq!(client.spawned(), ROUTER + CLOSE, "the client's runtime was handed {} tasks", client.spawned());
    });
}

#[test]
fn a_client_link_connects_on_the_runtime_it_is_handed() {
    OnSmol::block_on(async {
        let client = Counting::new();
        let handed: Arc<dyn Runtime> = Arc::new(client.clone());
        serve_and_call(Tcp::new("127.0.0.1:0"), Counting::new(), move |addr| async move {
            let outbound = Tcp::new(addr).connect(&handed).await.expect("a link given no runtime connects on the one handed to it");
            drop(outbound);
        })
        .await;
        assert_eq!(client.spawned(), LINK_TASKS, "the handed runtime was given {} tasks", client.spawned());
    });
}

/// Serves `link` on an app whose runtime is `app_runtime`, runs `client` against its address, and
/// stops.
async fn serve_and_call<C, F>(link: Tcp, app_runtime: Counting, client: C)
where
    C: FnOnce(std::net::SocketAddr) -> F,
    F: std::future::Future<Output = ()>,
{
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
    client(addr).await;
    let _ = handle.close(Signal::new("runtime")).await;
    let _ = serving.await;
}

/// One call from a client given its own runtime.
async fn one_call(addr: std::net::SocketAddr) {
    let client = RpcClient::new(Tcp::new(addr), Arc::new(OnSmol::runtime())).expect("the client's link is usable");
    assert_eq!(client.request::<_, u32>("runtime.add", &41u32).await.expect("the call is answered"), 42);
}

#[test]
fn a_server_link_given_no_runtime_runs_its_tasks_on_its_app_s() {
    OnSmol::block_on(async {
        let app = Counting::new();
        serve_and_call(Tcp::new("127.0.0.1:0"), app.clone(), one_call).await;
        // The accept loop, the forwarder, the connection, and its writer, beside the server's own.
        assert!(app.spawned() >= 4, "the app's runtime was handed {} tasks", app.spawned());
    });
}

#[test]
fn a_server_link_given_a_runtime_runs_its_tasks_there() {
    OnSmol::block_on(async {
        let (link, app) = (Counting::new(), Counting::new());
        serve_and_call(Tcp::new("127.0.0.1:0").with_runtime(link.clone()), app, one_call).await;
        // The accept loop, the forwarder, the connection, and its writer.
        assert_eq!(link.spawned(), 4, "the link's runtime was handed {} tasks", link.spawned());
    });
}
