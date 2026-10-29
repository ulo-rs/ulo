use std::net::SocketAddr;
use std::sync::Arc;

use ex_25_rpc_basics::{Divide, Journal, builder};
use ex_support::rpc::{bounded, eventually};
use ex_support::{Served, serve};
use ulo_rpc::{RpcClient, Server};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::ErrorKind;

async fn start() -> Served {
    let app = builder().runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    serve(app.bind(Server::new(Tcp::new("127.0.0.1:0"))).listen().await.expect("listens"))
}

fn client(addr: SocketAddr) -> RpcClient {
    RpcClient::new(Tcp::new(addr), Arc::new(Tokio::current())).expect("the link is usable")
}

#[tokio::test]
async fn a_message_is_answered_with_the_handler_s_value() {
    let server = start().await;
    let answer = bounded(client(server.addr).request::<_, i64>("calc.divide", &Divide { dividend: 84, divisor: 2 })).await;
    assert_eq!(answer.expect("answered"), 42);
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_handler_s_error_arrives_with_its_kind_and_message() {
    let server = start().await;
    let error = bounded(client(server.addr).request::<_, i64>("calc.divide", &Divide { dividend: 1, divisor: 0 })).await.expect_err("refused");
    assert_eq!((error.kind(), error.message()), (ErrorKind::Unprocessable, "the divisor is zero"));
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn an_event_reaches_its_handler_and_answers_nobody() {
    let server = start().await;
    let journal = server.handle.get::<Journal>().await.expect("the journal resolves");
    bounded(client(server.addr).emit("calc.note", "carry the one")).await.expect("published");
    eventually("the event's handler", || journal.entries() == ["carry the one"]).await;
    server.stop().await.expect("closes");
}

#[tokio::test]
async fn a_pattern_nothing_handles_is_refused_with_its_reason() {
    let server = start().await;
    let error = bounded(client(server.addr).request::<_, i64>("calc.nobody", &())).await.expect_err("refused");
    assert_eq!((error.kind(), error.reason()), (ErrorKind::Unavailable, Some("pattern_unhandled")));
    server.stop().await.expect("closes");
}
