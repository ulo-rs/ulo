//! An RPC-only app serves itself in a test through `ulo::testing::TestServer`, and this crate's
//! dependency tree holds no HTTP crate: the server is the one `ulo_rpc::Server` over the TCP link,
//! and the runtime is `ulo_runtime_tokio`'s.

use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ulo::testing::TestApp;
use ulo::{Bound, controller, handlers, module};
use ulo_rpc::{Payload, RpcClient};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_runtime_tokio::testing::start;
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

#[controller]
struct Adder;

#[handlers]
impl Adder {
    #[ulo_rpc::message("test_server.add")]
    async fn add(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0 + 1)
    }
}

#[module(controllers = [Adder])]
struct AppModule;

#[tokio::test]
async fn an_rpc_app_serves_itself_without_an_http_crate() {
    let served = start(TestApp::of(AppModule), ulo_rpc::Server::new(Tcp::new("127.0.0.1:0"))).await.expect("starts");
    let client = RpcClient::new(Tcp::new(served.addr()), Arc::new(Tokio::current()))
        .expect("the link is usable")
        .timeout(Bound::After(Duration::from_secs(5)))
        .expect("a nonzero timeout is taken");

    let answer: u32 = client.request("test_server.add", &41u32).await.expect("the call is answered");
    assert_eq!(answer, 42);

    served.close().await.expect("close is clean");
}
