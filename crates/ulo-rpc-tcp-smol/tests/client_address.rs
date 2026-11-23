//! What a TCP link on `async-io`'s sockets takes as its endpoint: an IP address and port, or a host
//! name and port, which it resolves through the system resolver as the runtime's blocking work
//! (F416). Text that is neither is refused where the client takes the link, as `RpcClient::new`'s
//! `UnusableLink`, not as every call's `Unavailable` (F415). A name resolving to no address fails a
//! server's `listen()` as `Configure` and a client's call `Unavailable`, each naming it.
//!
//! `localhost` is the host name the tests resolve, through `/etc/hosts`, and `nosuch.invalid` the
//! one that resolves to nothing: RFC 6761 reserves `.invalid` so that no resolver answers it.

use std::sync::Arc;
use std::time::Duration;

use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, StartupError, controller, handlers};
use ulo_rpc::{Payload, RpcClient, Server};
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;
use ulo_transport::{Classify, ErrorKind};

/// How long a call waits before it fails the test.
const PATIENCE: Duration = Duration::from_secs(5);

#[test]
fn a_client_given_text_naming_no_address_is_refused_where_it_is_built() {
    OnSmol::block_on(async {
        for written in ["billing", "billing:70000"] {
            let refused = RpcClient::new(Tcp::new(written), Arc::new(OnSmol::runtime())).map(drop).expect_err("a client on text naming no address");
            assert_eq!(refused.link(), "tcp");
            let refused = refused.to_string();
            assert!(
                refused.contains("is neither a socket address") && refused.contains("nor a host name and port") && refused.contains(written),
                "the refusal does not say `{written}` is neither an address nor a host name and port: {refused}"
            );
        }
        for written in ["10.0.0.5:7000", "[::1]:7000", "billing:7000"] {
            RpcClient::new(Tcp::new(written), Arc::new(OnSmol::runtime())).unwrap_or_else(|refused| panic!("a client on `{written}` was refused: {refused}"));
        }
    });
}

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

#[controller]
struct Adder;

#[handlers]
impl Adder {
    #[ulo_rpc::message("client_address.add")]
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

/// A server listening on `endpoint`, and a client on `client(port)` calling it.
async fn answered(endpoint: &str, client: impl Fn(u16) -> String) {
    let server = App::builder(ServerRoot)
        .runtime(OnSmol::runtime())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(Server::new(Tcp::new(endpoint)))
        .listen()
        .await
        .unwrap_or_else(|refused| panic!("a server on `{endpoint}` did not listen: {refused}"));
    let port = match server.addresses().as_slice() {
        [bound] => bound.addr.port(),
        other => panic!("expected one bound address, got {other:?}"),
    };
    let handle = server.handle();
    let serving = ulo::spawn_with(&OnSmol::runtime(), server.serve(std::future::pending::<Signal>()));
    let written = client(port);
    let rpc = RpcClient::new(Tcp::new(written.as_str()), Arc::new(OnSmol::runtime())).expect("the client is built");
    let answer = rpc.request::<_, u32>("client_address.add", &41).timeout(PATIENCE).await;
    assert!(matches!(answer, Ok(42)), "a client on `{written}` calling a server on `{endpoint}` was answered {answer:?}");
    let _ = handle.close(Signal::new("client_address")).await;
    let _ = serving.await;
}

#[test]
fn a_host_name_is_resolved_on_both_sides() {
    OnSmol::block_on(answered("localhost:0", |port| format!("localhost:{port}")));
}

#[test]
fn an_ipv4_and_an_ipv6_address_still_connect() {
    OnSmol::block_on(async {
        answered("127.0.0.1:0", |port| format!("127.0.0.1:{port}")).await;
        answered("[::1]:0", |port| format!("[::1]:{port}")).await;
    });
}

#[test]
fn a_server_on_a_name_resolving_to_nothing_is_refused_at_listen() {
    OnSmol::block_on(async {
        let refused = App::builder(ServerRoot)
            .runtime(OnSmol::runtime())
            .wire()
            .expect("the server wires")
            .connect()
            .await
            .expect("the server connects")
            .bind(Server::new(Tcp::new("nosuch.invalid:0")))
            .listen()
            .await
            .err()
            .expect("a server on a name resolving to nothing listened");
        assert!(matches!(refused, StartupError::Configure(_)), "{refused:?}");
        let text = refused.to_string();
        assert!(text.contains("`nosuch.invalid:0` resolved to no address"), "the refusal does not name the host: {text}");
    });
}

#[test]
fn a_client_on_a_name_resolving_to_nothing_fails_each_call_unavailable() {
    OnSmol::block_on(async {
        let rpc = RpcClient::new(Tcp::new("nosuch.invalid:7000"), Arc::new(OnSmol::runtime())).expect("a client on a well-formed host name is built");
        for _ in 0..2 {
            let failed = rpc.request::<_, u32>("client_address.add", &1).timeout(PATIENCE).await.expect_err("a call to a name resolving to nothing");
            assert_eq!(failed.kind(), ErrorKind::Unavailable, "{failed:?}");
            assert!(failed.message().contains("`nosuch.invalid:7000` resolved to no address"), "the failure does not name the host: {failed:?}");
        }
    });
}
