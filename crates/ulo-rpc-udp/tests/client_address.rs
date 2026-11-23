//! What a UDP link takes as its endpoint: an IP address and port, or a host name and port, which it
//! resolves through the system resolver (F416). Text that is neither is refused where the client
//! takes the link, as `RpcClient::new`'s `UnusableLink` or the client module's failed `connect`, not
//! as every call's `Unavailable` (F418). A name resolving to no address fails a server's `listen()`
//! and a client's call `Unavailable`, each naming it.
//!
//! `localhost` is the host name the tests resolve, through `/etc/hosts`, on both sides, so the
//! server's first address is the one the client tries first; `nosuch.invalid` is the one that
//! resolves to nothing: RFC 6761 reserves `.invalid` so that no resolver answers it.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ulo::{App, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Signal, StartupError, controller, handlers};
use ulo_rpc::{Payload, RpcClient, RpcClientModule, Server};
use ulo_rpc_udp::Udp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, ErrorKind};

/// How long a call waits before it fails the test.
const PATIENCE: Duration = Duration::from_secs(5);

fn names_neither_form(refused: &str, written: &str) {
    assert!(
        refused.contains("is neither a socket address") && refused.contains("nor a host name and port") && refused.contains(written),
        "the refusal does not say `{written}` is neither an address nor a host name and port: {refused}"
    );
}

#[tokio::test]
async fn a_client_given_text_naming_no_address_is_refused_where_it_is_built() {
    for written in ["billing", "billing:70000"] {
        let refused = RpcClient::new(Udp::new(written), Arc::new(Tokio::current())).map(drop).expect_err("a client on text naming no address");
        assert_eq!(refused.link(), "udp");
        names_neither_form(&refused.to_string(), written);
    }
}

#[tokio::test]
async fn a_client_given_an_address_or_a_host_name_and_port_is_built() {
    for written in ["10.0.0.5:7000", "[::1]:7000", "billing:7000"] {
        RpcClient::new(Udp::new(written), Arc::new(Tokio::current())).unwrap_or_else(|refused| panic!("a client on `{written}` was refused: {refused}"));
    }
}

struct ClientRoot(Mutex<Option<Udp>>);

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

#[tokio::test]
async fn a_client_module_given_text_naming_no_address_fails_connect() {
    let wired = App::builder(ClientRoot(Mutex::new(Some(Udp::new("billing"))))).runtime(Tokio::current()).wire().expect("the client wires");
    match wired.connect().await {
        Err(StartupError::Connect { error, .. }) => {
            let ConnectError::Hook { reason: FailureReason::Errored(refused), .. } = *error else {
                panic!("expected the client module's init hook to fail `connect`, got: {error}");
            };
            names_neither_form(&refused.to_string(), "billing");
        }
        Err(other) => panic!("expected the client module's init hook to fail `connect`, got: {other}"),
        Ok(app) => {
            let _ = app.close(Signal::new("client_address")).await;
            panic!("a client module over `billing` connected");
        }
    }
}

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
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(Server::new(Udp::new(endpoint)))
        .listen()
        .await
        .unwrap_or_else(|refused| panic!("a server on `{endpoint}` did not listen: {refused}"));
    let port = match server.addresses().as_slice() {
        [bound] => bound.addr.port(),
        other => panic!("expected one bound address, got {other:?}"),
    };
    let handle = server.handle();
    let serving = tokio::spawn(async move {
        let _ = server.serve(std::future::pending::<Signal>()).await;
    });
    let written = client(port);
    let rpc = RpcClient::new(Udp::new(written.as_str()), Arc::new(Tokio::current())).expect("the client is built");
    let answer = rpc.request::<_, u32>("client_address.add", &41).timeout(PATIENCE).await;
    assert!(matches!(answer, Ok(42)), "a client on `{written}` calling a server on `{endpoint}` was answered {answer:?}");
    let _ = handle.close(Signal::new("client_address")).await;
    let _ = serving.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_name_is_resolved_on_both_sides() {
    answered("localhost:0", |port| format!("localhost:{port}")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ipv4_and_an_ipv6_address_still_connect() {
    answered("127.0.0.1:0", |port| format!("127.0.0.1:{port}")).await;
    answered("[::1]:0", |port| format!("[::1]:{port}")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_on_a_name_resolving_to_nothing_is_refused_at_listen() {
    let refused = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(Server::new(Udp::new("nosuch.invalid:0")))
        .listen()
        .await
        .err()
        .expect("a server on a name resolving to nothing listened");
    assert!(matches!(refused, StartupError::Configure(_)), "{refused:?}");
    let text = refused.to_string();
    assert!(text.contains("`nosuch.invalid:0` resolved to no address"), "the refusal does not name the host: {text}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_on_a_name_resolving_to_nothing_fails_each_call_unavailable() {
    let rpc = RpcClient::new(Udp::new("nosuch.invalid:7000"), Arc::new(Tokio::current())).expect("a client on a well-formed host name is built");
    for _ in 0..2 {
        let failed = rpc.request::<_, u32>("client_address.add", &1).timeout(PATIENCE).await.expect_err("a call to a name resolving to nothing");
        assert_eq!(failed.kind(), ErrorKind::Unavailable, "{failed:?}");
        assert!(failed.message().contains("`nosuch.invalid:7000` resolved to no address"), "the failure does not name the host: {failed:?}");
    }
}

/// A name with an empty label other than the one a trailing dot ends can never resolve, so it is
/// refused where the link is set up: a client's `RpcClient::new`, as `UnusableLink`, and a server's
/// `listen()`, as `Configure`, before any lookup or socket.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_with_an_empty_label_is_refused_where_the_link_is_set_up() {
    for written in [".:7000", "..:7000", "a..b:7000", ".a:7000"] {
        let refused = RpcClient::new(Udp::new(written), Arc::new(Tokio::current())).map(drop).expect_err("a client on a name with an empty label");
        assert!(refused.to_string().contains(&format!("`{written}`")) && refused.to_string().contains("empty label"), "{refused}");
    }
    let refused = App::builder(ServerRoot)
        .runtime(Tokio::current())
        .wire()
        .expect("the server wires")
        .connect()
        .await
        .expect("the server connects")
        .bind(Server::new(Udp::new("a..b:0")))
        .listen()
        .await
        .err()
        .expect("a server on a name with an empty label listened");
    assert!(matches!(refused, StartupError::Configure(_)), "{refused:?}");
    assert!(refused.to_string().contains("`a..b:0`") && refused.to_string().contains("empty label"), "{refused}");
}
