use std::net::SocketAddr;
use std::sync::Arc;

use ex_41_one_guard_two_transports::builder;
use ex_support::http::{Call, call};
use ex_support::rpc::bounded;
use ex_support::{Served, serve};
use serde_json::Value;
use ulo_http_hyper::Server as Http;
use ulo_rpc::{RpcClient, Server as Rpc};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::ErrorKind;

struct Both {
    served: Served,
    rpc: SocketAddr,
}

async fn start() -> Both {
    let app = builder().runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    let app = app.bind(Http::new("127.0.0.1:0")).bind(Rpc::new(Tcp::new("127.0.0.1:0"))).listen().await.expect("listens");
    let rpc = app.addresses().iter().find(|bound| bound.transport == "rpc").expect("the rpc server bound").addr;
    Both { served: serve(app), rpc }
}

fn client(addr: SocketAddr) -> RpcClient {
    RpcClient::new(Tcp::new(addr), Arc::new(Tokio::current())).expect("the link is usable")
}

#[tokio::test]
async fn one_guard_admits_the_open_handler_on_both_transports() {
    let both = start().await;

    let http = call(both.served.addr, Call::get("/shop")).await;
    assert_eq!((http.status, http.body.as_str()), (200, "open"));
    let rpc = bounded(client(both.rpc).request::<_, String>("shop.open", &())).await;
    assert_eq!(rpc.expect("admitted"), "open");
    both.served.stop().await.expect("closes clean");
}

#[tokio::test]
async fn the_same_refusal_is_a_403_problem_over_http_and_a_forbidden_error_over_rpc() {
    let both = start().await;

    let http = call(both.served.addr, Call::get("/shop/stocktake")).await;
    assert_eq!(http.status, 403);
    assert_eq!(http.header("content-type"), Some("application/problem+json"));
    let problem: Value = serde_json::from_str(&http.body).unwrap_or_else(|e| panic!("{e}: {}", http.body));
    assert_eq!((problem["status"].as_u64(), problem["title"].as_str(), problem["detail"].as_str()), (Some(403), Some("Forbidden"), Some("forbidden")));

    let rpc = bounded(client(both.rpc).request::<_, String>("shop.stocktake", &())).await.expect_err("refused");
    assert_eq!((rpc.kind(), rpc.message()), (ErrorKind::Forbidden, "forbidden"));
    both.served.stop().await.expect("closes clean");
}
