use std::sync::Arc;

use ex_support::rpc::bounded;
use ex_support::{Bin, get};
use ulo_rpc::RpcClient;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

fn free_port() -> String {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port().to_string()
}

#[tokio::test]
async fn the_binary_guards_both_transports_and_exits_clean_on_sigterm() {
    let rpc_port = free_port();
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-41-one-guard-two-transports"), &[("RPC_PORT", rpc_port.as_str())]);
    let http = bin.addr();
    let line = bin.next_line();
    let rpc: std::net::SocketAddr = line.strip_prefix("listening on tcp://").unwrap_or_else(|| panic!("`{line}` is not a tcp line")).parse().unwrap();
    assert_eq!(get(http, "/shop").await.body, "open");
    assert_eq!(get(http, "/shop/stocktake").await.status, 403);
    let client = RpcClient::new(Tcp::new(rpc), Arc::new(Tokio::current())).expect("the link is usable");
    assert_eq!(bounded(client.request::<_, String>("shop.open", &())).await.expect("admitted"), "open");
    drop(client);
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
