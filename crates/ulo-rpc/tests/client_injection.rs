//! `RpcClientModule` as the way a provider reaches another service: a provider reads the client
//! through `Dep<RpcClient>`, two keyed client modules bind two clients that each reach their own
//! server, two unkeyed ones are ambiguous where a provider reads the client. Capability: RPC client
//! injection (G20).

use std::time::Duration;

use ulo::{App, AppHandle, Dep, Key, Module, ModuleDef, ModuleIdentity, Signal, StartupError, WiringError, injectable, routes};
use ulo_rpc::{Payload, RpcClient, RpcClientModule, RpcError};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);

#[injectable]
struct AddOne;

#[routes]
impl AddOne {
    #[ulo_rpc::message("inj.add")]
    fn add(&self, n: Payload<u32>) -> u32 {
        n.0 + 1
    }
}

#[injectable]
struct AddHundred;

#[routes]
impl AddHundred {
    #[ulo_rpc::message("inj.add")]
    fn add(&self, n: Payload<u32>) -> u32 {
        n.0 + 100
    }
}

struct ServerOne;

impl Module for ServerOne {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<AddOne>();
    }
}

struct ServerHundred;

impl Module for ServerHundred {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<AddHundred>();
    }
}

struct Server {
    addr: std::net::SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn serve(root: impl Module) -> Server {
    let app = App::builder(root)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Server { addr, handle, serving }
}

impl Server {
    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

async fn add(client: &RpcClient, n: u32) -> Result<u32, RpcError> {
    tokio::time::timeout(PATIENCE, client.request::<u32, u32>("inj.add", &n)).await.expect("the call answered in time")
}

/// A provider that calls through the client it is injected with.
#[injectable]
struct Forwarder {
    client: Dep<RpcClient>,
}

impl Forwarder {
    async fn forward(&self, n: u32) -> Result<u32, RpcError> {
        add(&self.client, n).await
    }
}

struct Calling(std::net::SocketAddr);

impl Module for Calling {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Tcp::new(self.0)));
        m.provide::<Forwarder>();
    }
}

#[tokio::test]
async fn a_provider_reads_the_module_s_client_through_dep_and_its_call_reaches_the_server() {
    let server = serve(ServerOne).await;
    let app = App::builder(Calling(server.addr)).runtime(Tokio::current()).wire().expect("the client wires").connect().await.expect("the client connects");
    let forwarder = app.get::<Forwarder>().await.expect("the provider resolves");
    assert_eq!(forwarder.forward(41).await.expect("the call is answered"), 42);
    let _ = app.close(Signal::new("test")).await;
    server.stop().await;
}

struct Billing;
struct Search;

/// Reads the client keyed `Billing` and the one keyed `Search`.
#[injectable]
struct Callers {
    billing: Dep<RpcClient, Billing>,
    search: Dep<RpcClient, Search>,
}

struct TwoServices(std::net::SocketAddr, std::net::SocketAddr);

impl Module for TwoServices {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&(self.0, self.1))
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Tcp::new(self.0)).keyed::<Billing>());
        m.import(RpcClientModule::for_root(Tcp::new(self.1)).keyed::<Search>());
        m.provide::<Callers>();
    }
}

#[tokio::test]
async fn two_keyed_client_modules_bind_two_clients_each_reaching_its_own_server() {
    let one = serve(ServerOne).await;
    let hundred = serve(ServerHundred).await;
    let app = App::builder(TwoServices(one.addr, hundred.addr)).runtime(Tokio::current()).wire().expect("the client wires").connect().await.expect("the client connects");
    let callers = app.get::<Callers>().await.expect("the provider resolves");
    assert_eq!(add(&callers.billing, 1).await.expect("Billing answers"), 2, "the Billing client reached the server adding one");
    assert_eq!(add(&callers.search, 1).await.expect("Search answers"), 101, "the Search client reached the server adding a hundred");
    let _ = app.close(Signal::new("test")).await;
    one.stop().await;
    hundred.stop().await;
}

struct TwoUnkeyed(std::net::SocketAddr);

impl Module for TwoUnkeyed {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(RpcClientModule::for_root(Tcp::new(self.0)));
        m.import(RpcClientModule::for_root(Tcp::new(self.0)));
        m.provide::<Forwarder>();
    }
}

#[tokio::test]
async fn two_unkeyed_client_modules_are_ambiguous_for_a_provider_reading_the_client() {
    let addr: std::net::SocketAddr = "127.0.0.1:1".parse().expect("an address");
    let Err(StartupError::Wiring(errors)) = App::builder(TwoUnkeyed(addr)).runtime(Tokio::current()).wire() else {
        panic!("two unkeyed client modules wired beside a provider reading `RpcClient`");
    };
    let ambiguous: Vec<_> = errors
        .iter()
        .filter_map(|error| match error {
            WiringError::Ambiguous { key, consumer, sources, .. } => Some((key, consumer, sources)),
            _ => None,
        })
        .collect();
    assert_eq!(ambiguous.len(), 1, "{errors}");
    let (key, _consumer, sources) = ambiguous[0];
    assert_eq!(key.key(), Key::of::<RpcClient, ()>());
    assert_eq!(sources.len(), 2, "both client modules are named as sources");
}
