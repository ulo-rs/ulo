//! What a TCP client connects to on `async-io`'s sockets: an IP address and port. A host name, which
//! the link does not resolve, is refused where the client takes the link, as `RpcClient::new`'s
//! `UnusableLink`, not as every call's `Unavailable` (F415).

use std::sync::Arc;

use ulo_rpc::RpcClient;
use ulo_rpc_conformance::{Harness, OnSmol};
use ulo_rpc_tcp_smol::Tcp;

#[test]
fn a_client_given_a_host_name_is_refused_where_it_is_built() {
    OnSmol::block_on(async {
        let refused = RpcClient::new(Tcp::new("billing:7000"), Arc::new(OnSmol::runtime())).map(drop).expect_err("a client on a host name");
        assert_eq!(refused.link(), "tcp");
        let refused = refused.to_string();
        assert!(
            refused.contains("takes IP addresses only") && refused.contains("billing:7000"),
            "the refusal does not say the TCP link takes IP addresses only, naming `billing:7000`: {refused}"
        );
        RpcClient::new(Tcp::new("10.0.0.5:7000"), Arc::new(OnSmol::runtime())).expect("a client on an IP address and port");
    });
}
