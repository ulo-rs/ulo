//! The tokio and smol TCP links speak one wire: a client of either is answered by a server of the
//! other, a unary call and a streamed reply alike. The smol link's tasks run on the tokio runtime
//! here, its app's as a server and its client's as a client, `async-io` driving its sockets from its
//! own thread.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{Stream, StreamExt, stream};
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc::{Link, Payload, RpcClient};
use ulo_tokio::Tokio;
use ulo_transport::{Classify, ErrorKind};

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[injectable]
struct Counter;

#[routes]
impl Counter {
    #[ulo_rpc::message("interop.add")]
    async fn add(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0 + 1)
    }

    #[ulo_rpc::message("interop.count")]
    async fn count(&self, n: Payload<u32>) -> impl Stream<Item = Result<u32, Never>> {
        stream::iter((1..=n.0).map(Ok))
    }
}

struct ServerRoot;

impl Module for ServerRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Counter>();
    }
}

/// Serves `server` on tokio, calls it through a client over the link `client` builds for the
/// server's address, and stops.
async fn exchange<S: Link, C: Link>(server: S, client: impl FnOnce(std::net::SocketAddr) -> C) {
    let server = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(ulo_rpc::Server::new(server))
        .listen()
        .await
        .expect("the server binds");
    let addr = server.addresses()[0].addr;
    let handle = server.handle();
    let serving = tokio::spawn(server.serve(std::future::pending::<Signal>()));

    let rpc = RpcClient::new(client(addr), Arc::new(Tokio::current()))
        .expect("the client's link is usable")
        .timeout(Bound::After(Duration::from_secs(5)))
        .expect("a nonzero timeout is taken");
    assert_eq!(rpc.request::<_, u32>("interop.add", &41u32).await.expect("the unary call is answered"), 42);
    let items: Vec<u32> = rpc
        .stream::<_, u32>("interop.count", &3u32)
        .await
        .expect("the stream opens")
        .map(|item| item.expect("every item arrives"))
        .collect()
        .await;
    assert_eq!(items, vec![1, 2, 3]);
    drop(rpc);

    let _ = handle.close(Signal::new("interop")).await;
    let _ = serving.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_smol_client_is_answered_by_a_tokio_server() {
    exchange(ulo_rpc_tcp::Tcp::new("127.0.0.1:0"), ulo_rpc_tcp_smol::Tcp::new).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tokio_client_is_answered_by_a_smol_server() {
    exchange(ulo_rpc_tcp_smol::Tcp::new("127.0.0.1:0"), ulo_rpc_tcp::Tcp::new).await;
}
