use std::sync::Arc;

use ex_support::rpc::{bounded, spawn_tcp, tcp_addr};
use futures_util::stream;
use ulo_rpc::RpcClient;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn the_binary_serves_on_the_port_it_is_given_and_exits_clean_on_sigterm() {
    let (mut bin, port) = spawn_tcp(env!("CARGO_BIN_EXE_ex-26-rpc-streams"), &[]);
    let addr = tcp_addr(&mut bin, port);
    let client = RpcClient::new(Tcp::new(addr), Arc::new(Tokio::current())).expect("the link is usable");
    let answer = bounded(client.send_stream::<_, i64>("stream.sum", stream::iter([4i64, 5]))).await;
    assert_eq!(answer.expect("answered"), 9);
    drop(client);
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
