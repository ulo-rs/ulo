//! A global enhancer contributed by type is built with its dependencies on each transport and in
//! each role: one guard, one interceptor and one error handler per transport, each an
//! `#[injectable]` reading one dependency, run in that order by a failing call on HTTP, RPC over
//! TCP, WebSocket and gRPC. Without the dependency's provider, `wire()` refuses every one of the
//! twelve.

mod support;

use std::error::Error;
use std::fmt;
use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use ulo::{
    AnyErrorHandler, AnyGuard, AnyInterceptor, App, BoxError, Dep, ErrorHandler, Guard, Interceptor, Module, ModuleDef, ModuleIdentity, Next, Signal,
    StartupError, Transport, WiringError, controller, handlers, injectable,
};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{self, AddRequest, Sum};
use ulo_grpc::{Grpc, GrpcCx};
use ulo_http::{Http, HttpCx};
use ulo_rpc::{Payload, Rpc, RpcClient, RpcCx};
use ulo_rpc_tcp::Tcp;
use ulo_transport::Classify;
use ulo_ws::{Ws, WsCx};

use support::{Record, within};

/// What every handler fails with, so the error handlers run.
#[derive(Debug, Classify)]
#[classify(conflict)]
struct Clash;

impl fmt::Display for Clash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("clash")
    }
}

impl Error for Clash {}

/// The one dependency every global reads: the record each writes its name to as it runs.
#[derive(Clone)]
struct Seen(Record<&'static str>);

/// A guard admitting, an interceptor passing through and an error handler handing the error on,
/// each named for its transport and recording that name in [`Seen`].
macro_rules! globals {
    ($($transport:ty, $cx:ty: $guard:ident, $interceptor:ident, $handler:ident;)*) => {$(
        #[injectable]
        struct $guard {
            seen: Dep<Seen>,
        }

        impl Guard<$transport> for $guard {
            async fn can_activate(&self, _cx: &$cx) -> Result<bool, BoxError> {
                self.seen.0.push(stringify!($guard));
                Ok(true)
            }
        }

        #[injectable]
        struct $interceptor {
            seen: Dep<Seen>,
        }

        impl Interceptor<$transport> for $interceptor {
            async fn intercept(&self, _cx: &$cx, next: Next<'_, $transport>) -> Result<<$transport as Transport>::Reply, BoxError> {
                self.seen.0.push(stringify!($interceptor));
                next.run().await
            }
        }

        #[injectable]
        struct $handler {
            seen: Dep<Seen>,
        }

        impl ErrorHandler<$transport> for $handler {
            async fn handle(&self, err: BoxError, _cx: &$cx) -> Result<<$transport as Transport>::Reply, BoxError> {
                self.seen.0.push(stringify!($handler));
                Err(err)
            }
        }
    )*};
}

globals! {
    Http, HttpCx: HttpGuard, HttpInterceptor, HttpErrorHandler;
    Rpc, RpcCx: RpcGuard, RpcInterceptor, RpcErrorHandler;
    Ws, WsCx: WsGuard, WsInterceptor, WsErrorHandler;
    Grpc, GrpcCx: GrpcGuard, GrpcInterceptor, GrpcErrorHandler;
}

const EVERY_GLOBAL: [&str; 12] = [
    "HttpGuard",
    "HttpInterceptor",
    "HttpErrorHandler",
    "RpcGuard",
    "RpcInterceptor",
    "RpcErrorHandler",
    "WsGuard",
    "WsInterceptor",
    "WsErrorHandler",
    "GrpcGuard",
    "GrpcInterceptor",
    "GrpcErrorHandler",
];

#[controller]
struct Web;

#[handlers]
impl Web {
    #[ulo_http::get("/fail")]
    fn fail(&self) -> Result<&'static str, Clash> {
        Err(Clash)
    }
}

#[controller]
struct Bus;

#[handlers]
impl Bus {
    #[ulo_rpc::message("globals.fail")]
    fn fail(&self, _n: Payload<u32>) -> Result<u32, Clash> {
        Err(Clash)
    }
}

#[controller]
struct Chat;

#[handlers]
#[ulo_ws::gateway(path = "/ws", port = own)]
impl Chat {
    #[ulo_ws::message("fail")]
    fn fail(&self) -> Result<String, Clash> {
        Err(Clash)
    }
}

#[controller]
struct Counter;

#[handlers]
impl Counter {
    #[ulo_grpc::method(pb::counter::Add)]
    fn add(&self, _req: ulo_grpc::Message<AddRequest>) -> Result<Sum, Clash> {
        Err(Clash)
    }
}

/// The four controllers and the twelve globals, each by type; `Seen` only when given one.
struct Root(Option<Seen>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if let Some(seen) = &self.0 {
            m.value(seen.clone());
        }
        m.enhancer::<AnyGuard<Http>>().provide::<HttpGuard>(|a| a);
        m.enhancer::<AnyInterceptor<Http>>().provide::<HttpInterceptor>(|a| a);
        m.enhancer::<AnyErrorHandler<Http>>().provide::<HttpErrorHandler>(|a| a);
        m.enhancer::<AnyGuard<Rpc>>().provide::<RpcGuard>(|a| a);
        m.enhancer::<AnyInterceptor<Rpc>>().provide::<RpcInterceptor>(|a| a);
        m.enhancer::<AnyErrorHandler<Rpc>>().provide::<RpcErrorHandler>(|a| a);
        m.enhancer::<AnyGuard<Ws>>().provide::<WsGuard>(|a| a);
        m.enhancer::<AnyInterceptor<Ws>>().provide::<WsInterceptor>(|a| a);
        m.enhancer::<AnyErrorHandler<Ws>>().provide::<WsErrorHandler>(|a| a);
        m.enhancer::<AnyGuard<Grpc>>().provide::<GrpcGuard>(|a| a);
        m.enhancer::<AnyInterceptor<Grpc>>().provide::<GrpcInterceptor>(|a| a);
        m.enhancer::<AnyErrorHandler<Grpc>>().provide::<GrpcErrorHandler>(|a| a);
        m.controller::<Web>();
        m.controller::<Bus>();
        m.controller::<Chat>();
        m.controller::<Counter>();
    }
}

fn address(addresses: &[ulo::BoundAddr], transport: &str) -> SocketAddr {
    match addresses.iter().filter(|bound| bound.transport == transport).collect::<Vec<_>>().as_slice() {
        [bound] => bound.addr,
        other => panic!("expected one `{transport}` address, got {other:?}"),
    }
}

/// `GET /fail` on its own connection: the status line.
async fn http_fail(addr: SocketAddr) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("the HTTP server accepts");
    stream.write_all(b"GET /fail HTTP/1.1\r\nHost: globals\r\nConnection: close\r\n\r\n").await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read to its end");
    String::from_utf8_lossy(&answer).lines().next().unwrap_or_default().to_owned()
}

/// `fail` on a connection of its own: the error envelope it is answered with.
async fn ws_fail(addr: SocketAddr) -> Value {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("the WebSocket upgrade");
    socket.send(Message::text(json!({ "event": "fail", "id": 1 }).to_string())).await.expect("the message is sent");
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(text.as_str()).expect("a JSON frame"),
            Some(Ok(_)) => continue,
            other => panic!("expected the answer, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn a_global_by_type_runs_with_its_dependency_on_every_transport_in_every_role() {
    let seen = Seen(Record::new());
    let app = App::builder(Root(Some(seen.clone())))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addresses = app.addresses();
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });
    let ran = |from: usize| seen.0.snapshot().split_off(from);

    let status = within("the HTTP call", http_fail(address(&addresses, Http::KEY))).await;
    assert!(status.starts_with("HTTP/1.1 409"), "{status}");
    assert_eq!(ran(0), EVERY_GLOBAL[0..3], "the HTTP globals, in their order");

    let rpc = address(&addresses, Rpc::KEY);
    let client = RpcClient::new(Tcp::new(rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");
    let error = within("the RPC call", client.request::<u32, u32>("globals.fail", &1).timeout(Duration::from_secs(5)).into_future()).await.expect_err("the call fails");
    assert_eq!(error.kind(), ulo_transport::ErrorKind::Conflict, "{error:?}");
    assert_eq!(ran(3), EVERY_GLOBAL[3..6], "the RPC globals, in their order");

    let answer = within("the WebSocket call", ws_fail(address(&addresses, Ws::KEY))).await;
    assert_eq!(answer["error"]["kind"], json!("conflict"), "{answer}");
    assert_eq!(ran(6), EVERY_GLOBAL[6..9], "the WebSocket globals, in their order");

    let grpc = address(&addresses, Grpc::KEY);
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{grpc}")).expect("an endpoint").connect().await.expect("the gRPC channel");
    let status = within("the gRPC call", CounterClient::new(channel).add(AddRequest { a: 1, b: 2 })).await.expect_err("the call fails");
    assert_eq!(status.code(), tonic::Code::Aborted, "{status:?}");
    assert_eq!(ran(9), EVERY_GLOBAL[9..12], "the gRPC globals, in their order");

    within("the close", handle.close(Signal::new("test"))).await.expect("a clean shutdown");
    let _ = within("serve ending", serving).await;
}

#[test]
fn without_the_dependency_s_provider_wire_refuses_every_global_reading_it() {
    let errors = match App::builder(Root(None)).wire() {
        Err(StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("expected wiring errors, got {other}"),
        Ok(_) => panic!("the app wired without `Seen`"),
    };
    let mut consumers: Vec<&str> = errors
        .iter()
        .map(|error| match error {
            WiringError::Missing { key, consumer, .. } => {
                assert_eq!(key.key(), ulo::Key::of::<Seen, ()>(), "{error}");
                consumer.as_str()
            }
            other => panic!("expected only missing dependencies, got {other}"),
        })
        .collect();
    consumers.sort_unstable();
    let mut expected = EVERY_GLOBAL.to_vec();
    expected.sort_unstable();
    let named: Vec<&str> = expected.iter().map(|global| *consumers.iter().find(|consumer| consumer.contains(global)).unwrap_or_else(|| panic!("no refusal names `{global}`: {consumers:?}"))).collect();
    assert_eq!(named.len(), consumers.len(), "one refusal per global: {consumers:?}");
}
