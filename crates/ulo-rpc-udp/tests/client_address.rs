//! What a UDP client sends to: an IP address and port. A host name, which the link does not
//! resolve, is refused where the client takes the link, as `RpcClient::new`'s `UnusableLink` or the
//! client module's failed `connect`, not as every call's `Unavailable` (F418); a server's own
//! endpoint is checked in `prepare` as before.

use std::sync::{Arc, Mutex};

use ulo::{App, ConnectError, FailureReason, Module, ModuleDef, ModuleIdentity, Signal, StartupError};
use ulo_rpc::{RpcClient, RpcClientModule};
use ulo_rpc_udp::Udp;
use ulo_runtime_tokio::Tokio;

fn says_ip_addresses_only(refused: &str, written: &str) {
    assert!(
        refused.contains("takes IP addresses only and resolves no host name") && refused.contains(written),
        "the refusal does not say the UDP link takes IP addresses only, naming `{written}`: {refused}"
    );
}

#[tokio::test]
async fn a_client_given_a_host_name_is_refused_where_it_is_built() {
    let refused = RpcClient::new(Udp::new("billing:7000"), Arc::new(Tokio::current())).map(drop).expect_err("a client on a host name");
    assert_eq!(refused.link(), "udp");
    says_ip_addresses_only(&refused.to_string(), "billing:7000");
}

#[tokio::test]
async fn a_client_given_an_ip_address_and_port_is_built() {
    RpcClient::new(Udp::new("10.0.0.5:7000"), Arc::new(Tokio::current())).expect("a client on an IP address and port");
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
async fn a_client_module_given_a_host_name_fails_connect() {
    let wired = App::builder(ClientRoot(Mutex::new(Some(Udp::new("billing:7000"))))).runtime(Tokio::current()).wire().expect("the client wires");
    match wired.connect().await {
        Err(StartupError::Connect(ConnectError::Hook { reason: FailureReason::Errored(refused), .. })) => {
            says_ip_addresses_only(&refused.to_string(), "billing:7000");
        }
        Err(other) => panic!("expected the client module's init hook to fail `connect`, got: {other}"),
        Ok(app) => {
            let _ = app.close(Signal::new("client_address")).await;
            panic!("a client module over a host name connected");
        }
    }
}
