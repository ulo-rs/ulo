//! A declaration's entries run in the order written, whichever spelling each uses.
//!
//! `#[use_guards(A::new(..), B, |ctx| C::new(ctx))]` names a value, a token and a constructor, and
//! they run in the order written. These pin that order for guards on each transport, and on HTTP
//! the permuted order, interceptors, the tier order, the error chain's order, and that the closure
//! form builds per execution from its execution's context where the value form builds once.
//!
//! `TokenGuard` and `Marker` implement `Guard` for every context and serve all four transports; the
//! interceptors and error handlers are HTTP's.

#![allow(dead_code)]

use std::sync::Mutex;

use serial_test::serial;
use ulo::async_trait;
use ulo::dispatch::Items;
use ulo::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use ulo::grpc::{GrpcContext, GrpcHandlerResult};
use ulo::http::{Body, HttpContext, HttpError, HttpHandlerResult, HttpResponse};
use ulo::rpc::{RpcContext, RpcData, RpcHandlerResult};
use ulo::ws::{WsContext, WsHandlerResult, WsMessage};
use ulo::{controller, get, injectable, module, routes};
use ulo_macros::{
    grpc_methods, message_pattern, new, patterns, subscribe_message, subscriptions,
    use_error_handlers, use_guards, use_interceptors, websocket_gateway,
};

use crate::common::{NotServed, TestServer};

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(what: &str) {
    SEEN.lock().unwrap().push(what.to_string());
}

fn seen() -> Vec<String> {
    SEEN.lock().unwrap().clone()
}

fn clear() {
    SEEN.lock().unwrap().clear();
}

fn built(label: &str) -> usize {
    let key = format!("built:{label}");
    seen().iter().filter(|s| **s == key).count()
}

// ── the three spellings' guards ──────────────────────────────────────────────

/// The value and constructor spellings: carries the label chosen at the declaration site, and
/// records its construction so a test can count how many times a spelling built it.
struct Marker {
    label: &'static str,
}

impl Marker {
    fn new(label: &'static str) -> Self {
        record(&format!("built:{label}"));
        Self { label }
    }
}

macro_rules! marker_guard {
    ($ctx:ty) => {
        #[async_trait]
        impl Guard<$ctx> for Marker {
            async fn can_activate(&self, _ctx: &$ctx) -> bool {
                record(self.label);
                true
            }
        }
    };
}
marker_guard!(HttpContext);
marker_guard!(RpcContext);
marker_guard!(WsContext);
marker_guard!(GrpcContext);

/// The token spelling: resolved from DI under its type token.
#[injectable]
pub struct TokenGuard {}
impl TokenGuard {}

macro_rules! token_guard {
    ($ctx:ty) => {
        #[async_trait]
        impl Guard<$ctx> for TokenGuard {
            async fn can_activate(&self, _ctx: &$ctx) -> bool {
                record("token");
                true
            }
        }
    };
}
token_guard!(HttpContext);
token_guard!(RpcContext);
token_guard!(WsContext);
token_guard!(GrpcContext);

// ── interceptors, value/constructor and token ────────────────────────────────

struct MarkingInterceptor {
    label: &'static str,
}

impl MarkingInterceptor {
    fn new(label: &'static str) -> Self {
        Self { label }
    }
}

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for MarkingInterceptor {
    async fn intercept(
        &self,
        ctx: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        record(self.label);
        next.run(ctx).await
    }
}

#[injectable]
pub struct TokenInterceptor {}
impl TokenInterceptor {}

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for TokenInterceptor {
    async fn intercept(
        &self,
        ctx: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        record("token");
        next.run(ctx).await
    }
}

// ── error handlers, value and token: each claims and names itself ────────────

struct ClaimingHandler {
    label: &'static str,
}

impl ClaimingHandler {
    fn new(label: &'static str) -> Self {
        Self { label }
    }
}

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for ClaimingHandler {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        Some(Ok(HttpResponse::ok().text(self.label).build()))
    }
}

#[injectable]
pub struct TokenHandler {}
impl TokenHandler {}

#[async_trait]
impl ErrorHandler<HttpContext, HttpHandlerResult> for TokenHandler {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &HttpContext,
    ) -> Option<HttpHandlerResult> {
        Some(Ok(HttpResponse::ok().text("token").build()))
    }
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

#[controller("/written")]
pub struct WrittenController {}

#[routes]
#[use_guards(Marker::new("value"), TokenGuard, |_ctx| Marker::new("constructor"))]
#[use_interceptors(
    MarkingInterceptor::new("value"),
    TokenInterceptor,
    |_ctx| MarkingInterceptor::new("constructor")
)]
impl WrittenController {
    #[get("/")]
    fn get(&self) -> Body {
        record("handler");
        Body::text("ok".to_string())
    }
}

#[controller("/permuted")]
pub struct PermutedController {}

#[routes]
#[use_guards(|_ctx| Marker::new("constructor"), TokenGuard, Marker::new("value"))]
#[use_interceptors(
    |_ctx| MarkingInterceptor::new("constructor"),
    TokenInterceptor,
    MarkingInterceptor::new("value")
)]
impl PermutedController {
    #[get("/")]
    fn get(&self) -> Body {
        record("handler");
        Body::text("ok".to_string())
    }
}

/// A value on the controller and a token on the method: the controller's entry runs first.
#[controller("/tiers")]
pub struct TiersController {}

#[routes]
#[use_guards(Marker::new("controller"))]
impl TiersController {
    #[get("/")]
    #[use_guards(TokenGuard)]
    fn get(&self) -> Body {
        Body::text("ok".to_string())
    }
}

/// The chain consults the last declared first, so with `[value, token]` the token handler claims.
#[controller("/claims-written")]
pub struct ClaimsWrittenController {}

#[routes]
#[use_error_handlers(ClaimingHandler::new("value"), TokenHandler)]
impl ClaimsWrittenController {
    #[get("/")]
    fn get(&self) -> Result<Body, HttpError> {
        Err(HttpError::InternalServerError("fails".into()))
    }
}

#[controller("/claims-permuted")]
pub struct ClaimsPermutedController {}

#[routes]
#[use_error_handlers(TokenHandler, ClaimingHandler::new("value"))]
impl ClaimsPermutedController {
    #[get("/")]
    fn get(&self) -> Result<Body, HttpError> {
        Err(HttpError::InternalServerError("fails".into()))
    }
}

#[controller("/built")]
pub struct BuiltController {}

#[routes]
impl BuiltController {
    #[get("/value")]
    #[use_guards(Marker::new("shared"))]
    fn value(&self) -> Body {
        Body::text("ok".to_string())
    }

    #[get("/constructor")]
    #[use_guards(|_ctx| Marker::new("per-execution"))]
    fn constructor(&self) -> Body {
        Body::text("ok".to_string())
    }

    /// The closure reads the execution's context, with its parameter annotated.
    #[get("/from-context")]
    #[use_guards(|ctx: &HttpContext| PathRecorder(ctx.request().uri.path().to_string()))]
    fn from_context(&self) -> Body {
        Body::text("ok".to_string())
    }
}

/// Built by a closure from the request it guards; records that request's path.
struct PathRecorder(String);

#[async_trait]
impl Guard<HttpContext> for PathRecorder {
    async fn can_activate(&self, _ctx: &HttpContext) -> bool {
        record(&format!("path:{}", self.0));
        true
    }
}

#[module(
    controllers: [
        WrittenController,
        PermutedController,
        TiersController,
        ClaimsWrittenController,
        ClaimsPermutedController,
        BuiltController,
    ],
    providers: [TokenGuard, TokenInterceptor, TokenHandler],
)]
impl HttpOrderModule {}

async fn http_get(server: &TestServer, path: &str) -> (u16, String) {
    let response = server.client().get(server.url(path)).send().await.unwrap();
    let status = response.status().as_u16();
    (status, response.text().await.unwrap())
}

#[serial]
#[tokio::test]
async fn http_guards_and_interceptors_run_in_the_order_written() {
    clear();
    let server = TestServer::start(HttpOrderModule).await;

    let (status, _) = http_get(&server, "/written/").await;
    assert_eq!(status, 200);

    let ran: Vec<String> = seen()
        .into_iter()
        .filter(|s| !s.starts_with("built:"))
        .collect();
    assert_eq!(
        ran,
        vec![
            "value",
            "token",
            "constructor",
            "value",
            "token",
            "constructor",
            "handler"
        ],
        "guards, then interceptors, each in the order written"
    );
}

#[serial]
#[tokio::test]
async fn http_permuting_the_spellings_permutes_the_order() {
    clear();
    let server = TestServer::start(HttpOrderModule).await;

    let (status, _) = http_get(&server, "/permuted/").await;
    assert_eq!(status, 200);

    let ran: Vec<String> = seen()
        .into_iter()
        .filter(|s| !s.starts_with("built:"))
        .collect();
    assert_eq!(
        ran,
        vec![
            "constructor",
            "token",
            "value",
            "constructor",
            "token",
            "value",
            "handler"
        ]
    );
}

#[serial]
#[tokio::test]
async fn http_a_controller_value_runs_before_a_method_token() {
    clear();
    let server = TestServer::start(HttpOrderModule).await;

    let (status, _) = http_get(&server, "/tiers/").await;
    assert_eq!(status, 200);

    let ran: Vec<String> = seen()
        .into_iter()
        .filter(|s| !s.starts_with("built:"))
        .collect();
    assert_eq!(ran, vec!["controller", "token"]);
}

#[serial]
#[tokio::test]
async fn http_error_handlers_are_consulted_in_reverse_of_the_order_written() {
    clear();
    let server = TestServer::start(HttpOrderModule).await;

    let (_, body) = http_get(&server, "/claims-written/").await;
    assert_eq!(body, "token", "declared last, consulted first");

    let (_, body) = http_get(&server, "/claims-permuted/").await;
    assert_eq!(body, "value", "declared last, consulted first");
}

#[serial]
#[tokio::test]
async fn http_a_closure_builds_per_execution_and_a_value_does_not() {
    clear();
    let server = TestServer::start(HttpOrderModule).await;

    assert_eq!(built("shared"), 1, "the value form is built at startup");
    let per_execution_before = built("per-execution");

    for path in [
        "/built/value",
        "/built/value",
        "/built/constructor",
        "/built/constructor",
    ] {
        let (status, _) = http_get(&server, path).await;
        assert_eq!(status, 200, "{path}");
    }

    assert_eq!(built("shared"), 1, "the value form is shared, not rebuilt");
    assert_eq!(
        seen().iter().filter(|s| *s == "shared").count(),
        2,
        "the shared value ran for both requests"
    );
    assert_eq!(
        built("per-execution") - per_execution_before,
        2,
        "the closure form is built once per execution"
    );
}

#[serial]
#[tokio::test]
async fn http_a_closure_is_given_its_executions_context() {
    clear();
    let server = TestServer::start(HttpOrderModule).await;

    let (status, _) = http_get(&server, "/built/from-context").await;
    assert_eq!(status, 200);
    assert!(
        seen().contains(&"path:/built/from-context".to_string()),
        "{:?}",
        seen()
    );
}

// ── RPC ──────────────────────────────────────────────────────────────────────

#[controller]
pub struct OrderRpcController {}

#[patterns]
#[use_guards(Marker::new("value"), TokenGuard, |_ctx| Marker::new("constructor"))]
impl OrderRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("order.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        record("handler");
        Ok(Items::One(RpcData::json(serde_json::json!({"ok": true}))))
    }
}

#[module(controllers: [OrderRpcController], providers: [TokenGuard])]
impl RpcOrderModule {}

async fn rpc_boot() -> u16 {
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = ulo::UloFactory::create(RpcOrderModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    port_rx.await.expect("RPC server failed to bind")
}

async fn rpc_call(port: u16, pattern: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{port}"))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reader.read_line(&mut line),
    )
    .await
    .expect("a reply must arrive")
    .expect("the connection must stay readable");
    serde_json::from_str(&line).expect("the reply must be JSON")
}

#[serial]
#[tokio::test]
async fn rpc_guards_run_in_the_order_written() {
    clear();
    let port = rpc_boot().await;

    rpc_call(port, "order.echo").await;

    let ran: Vec<String> = seen()
        .into_iter()
        .filter(|s| !s.starts_with("built:"))
        .collect();
    assert_eq!(ran, vec!["value", "token", "constructor", "handler"]);
}

// ── WebSocket ────────────────────────────────────────────────────────────────

#[websocket_gateway("/order-ws")]
pub struct OrderGateway {}

#[subscriptions]
impl OrderGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// On the handler rather than the gateway, so the guards run once per message and not on
    /// the connect as well.
    #[subscribe_message("echo")]
    #[use_guards(Marker::new("value"), TokenGuard, |_ctx| Marker::new("constructor"))]
    async fn echo(&self) -> WsHandlerResult {
        record("handler");
        Ok(WsMessage::text("echo").into())
    }
}

#[module(providers: [OrderGateway, TokenGuard])]
pub struct WsOrderModule;

#[serial]
#[tokio::test]
async fn ws_guards_run_in_the_order_written() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    clear();
    let server = TestServer::start(WsOrderModule).await;

    let url = format!("ws://127.0.0.1:{}/order-ws", server.port);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let frame = serde_json::json!({"event": "echo"}).to_string();
    socket.send(Message::Text(frame.into())).await.unwrap();
    match tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => assert_eq!(t.as_str(), "echo"),
        other => panic!("expected a text reply, got {other:?}"),
    }

    let ran: Vec<String> = seen()
        .into_iter()
        .filter(|s| !s.starts_with("built:"))
        .collect();
    assert_eq!(ran, vec!["value", "token", "constructor", "handler"]);
}

// ── gRPC ─────────────────────────────────────────────────────────────────────

mod order_pb {
    tonic::include_proto!("ulo_test.orders");
}

use order_pb::orders_server::{Orders, OrdersServer};
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;

#[controller]
pub struct OrderGrpcService {}

#[grpc_methods(order_pb::orders_server::Orders)]
#[use_guards(Marker::new("value"), TokenGuard, |_ctx| Marker::new("constructor"))]
impl OrderGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<order_pb::CreateOrderRequest>,
    ) -> Result<order_pb::CreateOrderResponse, NotServed> {
        record("handler");
        Ok(order_pb::CreateOrderResponse {
            id: 1,
            status: "created".to_string(),
        })
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<order_pb::CreateOrderRequest>,
    ) -> Result<order_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<order_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<order_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<order_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<order_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [OrderGrpcService], providers: [TokenGuard])]
impl GrpcOrderModule {}

#[serial]
#[tokio::test]
async fn grpc_guards_run_in_the_order_written() {
    clear();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = ulo_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();
    tokio::spawn(async move {
        let mut app = ulo::UloFactory::create(GrpcOrderModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        let _ = sd_tx.send(app.shutdown_handle());
        app.run().await;
    });
    let port = port_rx.await.unwrap();
    let shutdown = sd_rx.await.unwrap();

    let mut client = order_pb::orders_client::OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    );
    client
        .create(order_pb::CreateOrderRequest {
            item: "book".into(),
            qty: 1,
        })
        .await
        .expect("the call must be served");
    shutdown.shutdown();

    let ran: Vec<String> = seen()
        .into_iter()
        .filter(|s| !s.starts_with("built:"))
        .collect();
    assert_eq!(ran, vec!["value", "token", "constructor", "handler"]);
}
