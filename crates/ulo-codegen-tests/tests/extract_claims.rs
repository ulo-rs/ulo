//! A parameter that fails to extract on RPC, WebSocket or gRPC is offered to the global error
//! handlers as the `ExtractError` it failed with, naming the parameter, and an error handler
//! claiming it answers the call in its place, as on HTTP (`ulo-http`'s `extract.rs`).

mod support;

use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use http::uri::PathAndQuery;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use ulo::{AnyErrorHandler, App, BoxError, ErrorHandler, Module, ModuleDef, ModuleIdentity, Signal, Transport, controller, handlers};
use ulo_codegen_tests::pb::AddRequest;
use ulo_codegen_tests::probe;
use ulo_grpc::{Grpc, GrpcCx};
use ulo_rpc::{Payload, Rpc, RpcClient, RpcCx};
use ulo_rpc_tcp::Tcp;
use ulo_transport::{CallError, ExtractError};
use ulo_ws::{Ws, WsCx};

use support::within;

/// `variant|param` for an `ExtractError`, the error arriving bare or as a `CallError`'s source.
fn extract_failure(err: &BoxError) -> Option<String> {
    let extract = err.downcast_ref::<ExtractError>().or_else(|| err.downcast_ref::<CallError>().and_then(|call| call.source_as::<ExtractError>()))?;
    Some(match extract {
        ExtractError::Missing { param } => format!("Missing|{param}"),
        ExtractError::Malformed { param, .. } => format!("Malformed|{param}"),
        ExtractError::Mismatched { param, .. } => format!("Mismatched|{param}"),
        other => format!("other|{other}"),
    })
}

/// Claims an extraction failure with an answer naming it; hands anything else on.
struct Claim;

impl ErrorHandler<Rpc> for Claim {
    async fn handle(&self, err: BoxError, cx: &RpcCx) -> Result<<Rpc as Transport>::Reply, BoxError> {
        match extract_failure(&err) {
            Some(failure) => Ok(cx.reply(&failure)?),
            None => Err(err),
        }
    }
}

impl ErrorHandler<Ws> for Claim {
    async fn handle(&self, err: BoxError, cx: &WsCx) -> Result<<Ws as Transport>::Reply, BoxError> {
        match extract_failure(&err) {
            Some(failure) => Ok(cx.reply(&failure)?),
            None => Err(err),
        }
    }
}

impl ErrorHandler<Grpc> for Claim {
    async fn handle(&self, err: BoxError, cx: &GrpcCx) -> Result<<Grpc as Transport>::Reply, BoxError> {
        match extract_failure(&err) {
            Some(failure) => Ok(cx.reply(failure)?),
            None => Err(err),
        }
    }
}

#[controller]
struct Bus;

#[handlers]
impl Bus {
    #[ulo_rpc::message("claims.double")]
    fn double(&self, n: Payload<u32>) -> String {
        (n.0 * 2).to_string()
    }
}

#[controller]
struct Chat;

#[handlers]
#[ulo_ws::gateway(path = "/ws", port = own)]
impl Chat {
    #[ulo_ws::message("double")]
    fn double(&self, n: ulo_ws::Payload<u32>) -> String {
        (n.0 * 2).to_string()
    }
}

#[controller]
struct Known;

#[handlers]
impl Known {
    #[ulo_grpc::method(probe::known::Upper)]
    fn upper(&self, text: ulo_grpc::Message<String>) -> String {
        text.0.to_uppercase()
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let rpc: Arc<AnyErrorHandler<Rpc>> = Arc::new(Claim);
        m.enhancer::<AnyErrorHandler<Rpc>>().value(rpc);
        let ws: Arc<AnyErrorHandler<Ws>> = Arc::new(Claim);
        m.enhancer::<AnyErrorHandler<Ws>>().value(ws);
        let grpc: Arc<AnyErrorHandler<Grpc>> = Arc::new(Claim);
        m.enhancer::<AnyErrorHandler<Grpc>>().value(grpc);
        m.controller::<Bus>();
        m.controller::<Chat>();
        m.controller::<Known>();
    }
}

fn address(addresses: &[ulo::BoundAddr], transport: &str) -> SocketAddr {
    match addresses.iter().filter(|bound| bound.transport == transport).collect::<Vec<_>>().as_slice() {
        [bound] => bound.addr,
        other => panic!("expected one `{transport}` address, got {other:?}"),
    }
}

/// `double` with `data` on a connection of its own: the answer's frame.
async fn ws_double(addr: SocketAddr, data: Value) -> Value {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("the WebSocket upgrade");
    socket.send(Message::text(json!({ "event": "double", "id": 1, "data": data }).to_string())).await.expect("the message is sent");
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(text.as_str()).expect("a JSON frame"),
            Some(Ok(_)) => continue,
            other => panic!("expected the answer, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn an_error_handler_claims_an_extraction_failure_on_rpc_websocket_and_grpc() {
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addresses = app.addresses();
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });

    let rpc = address(&addresses, Rpc::KEY);
    let client = RpcClient::new(Tcp::new(rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");
    let call = |data: Value| client.request::<Value, String>("claims.double", &data).timeout(Duration::from_secs(5)).into_future();
    assert_eq!(within("the RPC call", call(json!(4))).await.expect("the call is answered"), "8");
    assert_eq!(within("the RPC call", call(json!("four"))).await.expect("the failure is claimed"), "Malformed|payload");

    let ws = address(&addresses, Ws::KEY);
    assert_eq!(within("the WebSocket call", ws_double(ws, json!(4))).await, json!({ "id": 1, "data": "8" }));
    assert_eq!(within("the WebSocket call", ws_double(ws, json!("four"))).await, json!({ "id": 1, "data": "Malformed|data" }));

    let grpc = address(&addresses, Grpc::KEY);
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{grpc}")).expect("an endpoint").connect().await.expect("the gRPC channel");
    let mut known = probe::known_client::KnownClient::new(channel.clone());
    assert_eq!(within("the gRPC call", known.upper("ada".to_owned())).await.expect("the call is answered").into_inner(), "ADA");
    // An `AddRequest` writes field 1 as a varint, where a `StringValue` has a string.
    let mut raw = tonic::client::Grpc::new(channel);
    within("the channel's readiness", raw.ready()).await.unwrap_or_else(|error| panic!("not ready: {error}"));
    let path = PathAndQuery::from_static("/codegen.probe.v1.Known/Upper");
    let codec = tonic_prost::ProstCodec::<AddRequest, String>::default();
    let claimed = within("the gRPC call", raw.unary(tonic::Request::new(AddRequest { a: 1, b: 2 }), path, codec)).await.expect("the failure is claimed");
    assert_eq!(claimed.into_inner(), "Malformed|message");

    within("the close", handle.close(Signal::new("test"))).await.expect("a clean shutdown");
    let _ = within("serve ending", serving).await;
}
