//! `#[use_guards(MyGuard{})]` — the enhancer named as a value rather than as a DI token.
//!
//! HTTP has carried this form since the attribute existed. RPC, WebSocket and gRPC accepted the
//! syntax and registered nothing, so a guard written to deny compiled and the call was served.
//! These pin the form on the three transports that dropped it, at both tiers: named on the handler
//! impl, and named on one handler.

#![allow(dead_code)]

use std::sync::Mutex;
use std::time::Duration;

use serial_test::serial;
use ulo::UloFactory;
use ulo::async_trait;
use ulo::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
use ulo::module;
use ulo::rpc::{RpcContext, RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult};
use ulo_macros::{
    controller, message_pattern, new, patterns, use_error_handlers, use_guards, use_interceptors,
};

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(what: &str) {
    SEEN.lock().unwrap().push(what.to_string());
}

fn seen() -> Vec<String> {
    SEEN.lock().unwrap().clone()
}

/// Carries a field, so the value form is doing work a bare token could not: the label is
/// chosen at the declaration site.
struct MarkingGuard {
    label: &'static str,
}

#[async_trait]
impl Guard<RpcContext> for MarkingGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        record(self.label);
        true
    }
}

struct DenyingGuard;

#[async_trait]
impl Guard<RpcContext> for DenyingGuard {
    async fn can_activate(&self, _ctx: &RpcContext) -> bool {
        record("deny");
        false
    }
}

struct MarkingInterceptor {
    label: &'static str,
}

#[async_trait]
impl Interceptor<RpcContext, RpcHandlerResult> for MarkingInterceptor {
    async fn intercept(
        &self,
        ctx: &RpcContext,
        next: Box<dyn InterceptorNext<RpcContext, RpcHandlerResult>>,
    ) -> RpcHandlerResult {
        record(self.label);
        next.run(ctx).await
    }
}

struct ClaimingErrorHandler;

#[async_trait]
impl ErrorHandler<RpcContext, RpcHandlerResult> for ClaimingErrorHandler {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &RpcContext,
    ) -> Option<RpcHandlerResult> {
        record("claimed");
        Some(Ok(RpcData::from_serialize(
            &serde_json::json!({"claimed": true}),
        )
        .unwrap()
        .into()))
    }
}

#[controller]
pub struct InlineRpcController {}

#[patterns]
#[use_guards(MarkingGuard { label: "controller:guard" })]
#[use_interceptors(MarkingInterceptor { label: "controller:interceptor" })]
#[use_error_handlers(ClaimingErrorHandler {})]
impl InlineRpcController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("inline.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        record("handler");
        Ok(RpcHandlerOutput::Single(
            RpcData::from_serialize(&serde_json::json!({"ok": true})).unwrap(),
        ))
    }

    #[message_pattern("inline.fails")]
    async fn fails(&self) -> RpcHandlerResult {
        Err(RpcError::Internal("handler said no".into()))
    }
}

/// No error handler here. A guard rejection renders as the transport's own refusal rather than
/// something a sibling declaration reshapes, which is what the handler-level test reads.
#[controller]
pub struct InlineRpcStrictController {}

#[patterns]
impl InlineRpcStrictController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("inline.denied")]
    #[use_guards(DenyingGuard {})]
    async fn denied(&self) -> RpcHandlerResult {
        record("handler:denied");
        Ok(RpcHandlerOutput::Single(RpcData::json(serde_json::json!(
            {}
        ))))
    }
}

#[module(controllers: [InlineRpcController, InlineRpcStrictController])]
impl InlineRpcModule {}

async fn boot() -> u16 {
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = UloFactory::create(InlineRpcModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
}

async fn call(port: u16, pattern: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);

    let mut frame = serde_json::json!({"pattern": pattern, "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .expect("the connection must stay readable");
    serde_json::from_str(&line).expect("the reply must be JSON")
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_rpc_controller_s_inline_guard_and_interceptor_run() {
    SEEN.lock().unwrap().clear();
    let port = boot().await;

    call(port, "inline.echo").await;

    assert_eq!(
        seen(),
        vec!["controller:guard", "controller:interceptor", "handler"]
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_rpc_handler_s_inline_guard_rejects_the_call() {
    SEEN.lock().unwrap().clear();
    let port = boot().await;

    let reply = call(port, "inline.denied").await;

    assert_eq!(reply["err"]["status"], "forbidden", "reply: {reply}");
    assert_eq!(seen(), vec!["deny"], "the handler must not run");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_rpc_controller_s_inline_error_handler_claims_the_error() {
    SEEN.lock().unwrap().clear();
    let port = boot().await;

    let reply = call(port, "inline.fails").await;

    assert!(
        seen().contains(&"claimed".to_string()),
        "seen: {:?}",
        seen()
    );
    assert_eq!(reply["response"]["claimed"], true, "reply: {reply}");
}

// ── WebSocket ──────────────────────────────────────────────────────────────

mod ws {
    use super::{SEEN, record, seen};
    use ulo::async_trait;
    use ulo::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
    use ulo::module;
    use ulo::ws::{WsContext, WsError, WsHandlerResult, WsMessage};
    use ulo_macros::{
        new, subscribe_message, subscriptions, use_error_handlers, use_guards, use_interceptors,
        websocket_gateway,
    };

    pub struct MarkingGuard {
        pub label: &'static str,
    }

    #[async_trait]
    impl Guard<WsContext> for MarkingGuard {
        async fn can_activate(&self, _ctx: &WsContext) -> bool {
            record(self.label);
            true
        }
    }

    pub struct DenyingGuard;

    #[async_trait]
    impl Guard<WsContext> for DenyingGuard {
        async fn can_activate(&self, _ctx: &WsContext) -> bool {
            record("deny");
            false
        }
    }

    pub struct MarkingInterceptor {
        pub label: &'static str,
    }

    #[async_trait]
    impl Interceptor<WsContext, WsHandlerResult> for MarkingInterceptor {
        async fn intercept(
            &self,
            ctx: &WsContext,
            next: Box<dyn InterceptorNext<WsContext, WsHandlerResult>>,
        ) -> WsHandlerResult {
            record(self.label);
            next.run(ctx).await
        }
    }

    pub struct ClaimingErrorHandler;

    #[async_trait]
    impl ErrorHandler<WsContext, WsHandlerResult> for ClaimingErrorHandler {
        async fn handle_error(
            &self,
            _error: ChainError<'_>,
            _ctx: &WsContext,
        ) -> Option<WsHandlerResult> {
            record("claimed");
            Some(Ok(WsMessage::text("claimed").into()))
        }
    }

    #[websocket_gateway("/inline-ws")]
    pub struct InlineGateway {}

    #[subscriptions]
    #[use_guards(MarkingGuard { label: "gateway:guard" })]
    #[use_interceptors(MarkingInterceptor { label: "gateway:interceptor" })]
    #[use_error_handlers(ClaimingErrorHandler {})]
    impl InlineGateway {
        #[new]
        pub fn new() -> Self {
            Self {}
        }

        #[subscribe_message("echo")]
        async fn echo(&self) -> WsHandlerResult {
            record("handler");
            Ok(WsMessage::text("echo").into())
        }

        #[subscribe_message("fails")]
        async fn fails(&self) -> WsHandlerResult {
            Err(WsError::Internal("handler said no".into()))
        }
    }

    /// No error handler, so a rejection renders as the canonical refusal.
    #[websocket_gateway("/inline-ws-strict")]
    pub struct InlineStrictGateway {}

    #[subscriptions]
    impl InlineStrictGateway {
        #[new]
        pub fn new() -> Self {
            Self {}
        }

        #[subscribe_message("denied")]
        #[use_guards(DenyingGuard {})]
        async fn denied(&self) -> WsHandlerResult {
            record("handler:denied");
            Ok(WsMessage::text("denied").into())
        }
    }

    #[module(providers: [InlineGateway, InlineStrictGateway])]
    pub struct InlineWsModule;

    pub async fn boot() -> u16 {
        use ulo::UloFactory;
        use ulo_http_axum::AxumAdapter;

        let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            let mut app = UloFactory::create(InlineWsModule).await.unwrap();
            app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
                .unwrap();
            let bound = app.bind().await.unwrap();
            let _ = port_tx.send(bound.http.expect("HTTP must bind").port());
            app.run().await;
        });
        tokio::task::spawn_local(async move {
            local.await;
        });
        port_rx.await.unwrap()
    }

    pub async fn send(port: u16, path: &str, event: &str) -> String {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::Message;

        let url = format!("ws://127.0.0.1:{port}{path}");
        let (mut socket, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
        let frame = serde_json::json!({"event": event}).to_string();
        socket.send(Message::Text(frame.into())).await.unwrap();

        match tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => t.to_string(),
            other => panic!("expected a text reply, got {other:?}"),
        }
    }

    pub fn clear() {
        SEEN.lock().unwrap().clear();
    }
}

/// The guard appears twice because a gateway-level guard gates the upgrade and then every message
/// on it. The interceptor appears once: a connect has no call to wrap.
#[serial]
#[tokio_localset_test::localset_test]
async fn a_gateway_s_inline_guard_and_interceptor_run() {
    ws::clear();
    let port = ws::boot().await;

    ws::send(port, "/inline-ws", "echo").await;

    assert_eq!(
        seen(),
        vec![
            "gateway:guard",
            "gateway:guard",
            "gateway:interceptor",
            "handler"
        ]
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_ws_handler_s_inline_guard_rejects_the_message() {
    ws::clear();
    let port = ws::boot().await;

    let reply = ws::send(port, "/inline-ws-strict", "denied").await;

    assert!(
        reply.contains("orbidden") || reply.contains("403"),
        "reply: {reply}"
    );
    assert_eq!(seen(), vec!["deny"], "the handler must not run");
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_gateway_s_inline_error_handler_claims_the_error() {
    ws::clear();
    let port = ws::boot().await;

    let reply = ws::send(port, "/inline-ws", "fails").await;

    assert!(
        seen().contains(&"claimed".to_string()),
        "seen: {:?}",
        seen()
    );
    assert_eq!(reply, "claimed", "reply: {reply}");
}

// ── gRPC ───────────────────────────────────────────────────────────────────

mod grpc {
    use super::{SEEN, record};
    use crate::common::NotServed;
    use ulo::async_trait;
    use ulo::enhancer::{ChainError, ErrorHandler, Guard, Interceptor, InterceptorNext};
    use ulo::extract::Payload;
    use ulo::grpc::extract::Inbound;
    use ulo::grpc::{GrpcContext, GrpcHandlerResult, GrpcStatus};
    use ulo::module;
    use ulo_macros::{
        controller, grpc_methods, new, use_error_handlers, use_guards, use_interceptors,
    };

    pub mod pb {
        tonic::include_proto!("ulo_test.orders");
    }

    pub struct MarkingGuard {
        pub label: &'static str,
    }

    #[async_trait]
    impl Guard<GrpcContext> for MarkingGuard {
        async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
            record(self.label);
            true
        }
    }

    pub struct DenyingGuard;

    #[async_trait]
    impl Guard<GrpcContext> for DenyingGuard {
        async fn can_activate(&self, _ctx: &GrpcContext) -> bool {
            record("deny");
            false
        }
    }

    pub struct MarkingInterceptor {
        pub label: &'static str,
    }

    #[async_trait]
    impl Interceptor<GrpcContext, GrpcHandlerResult> for MarkingInterceptor {
        async fn intercept(
            &self,
            ctx: &GrpcContext,
            next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
        ) -> GrpcHandlerResult {
            record(self.label);
            next.run(ctx).await
        }
    }

    pub struct ClaimingErrorHandler;

    #[async_trait]
    impl ErrorHandler<GrpcContext, GrpcStatus> for ClaimingErrorHandler {
        async fn handle_error(
            &self,
            _error: ChainError<'_>,
            _ctx: &GrpcContext,
        ) -> Option<GrpcStatus> {
            record("claimed");
            Some(GrpcStatus::not_found("claimed"))
        }
    }

    #[controller]
    pub struct InlineGrpcService {}

    #[grpc_methods(pb::orders_server::Orders)]
    #[use_guards(MarkingGuard { label: "service:guard" })]
    #[use_interceptors(MarkingInterceptor { label: "service:interceptor" })]
    #[use_error_handlers(ClaimingErrorHandler {})]
    impl InlineGrpcService {
        #[new]
        pub fn new() -> Self {
            Self {}
        }

        #[grpc_method]
        async fn create(
            &self,
            Payload(_req): Payload<pb::CreateOrderRequest>,
        ) -> Result<pb::CreateOrderResponse, NotServed> {
            record("handler");
            Ok(pb::CreateOrderResponse {
                id: 1,
                status: "created".to_string(),
            })
        }

        /// Its own inline guard, on top of the service's.
        #[grpc_method]
        #[use_guards(DenyingGuard {})]
        async fn bulk_create(
            &self,
            _inbound: Inbound<pb::CreateOrderRequest>,
        ) -> Result<pb::BulkCreateResponse, NotServed> {
            record("handler:denied");
            Err(NotServed)
        }

        #[grpc_stream]
        async fn watch_progress(
            &self,
            Payload(_req): Payload<pb::WatchRequest>,
        ) -> Result<
            impl futures_util::Stream<Item = Result<pb::ProgressEvent, NotServed>> + Send + 'static,
            NotServed,
        > {
            Ok(futures_util::stream::empty())
        }

        #[grpc_stream]
        async fn chat(
            &self,
            _inbound: Inbound<pb::ChatMessage>,
        ) -> Result<
            impl futures_util::Stream<Item = Result<pb::ChatMessage, NotServed>> + Send + 'static,
            NotServed,
        > {
            Ok(futures_util::stream::empty())
        }
    }

    #[module(controllers: [InlineGrpcService])]
    impl InlineGrpcModule {}

    pub async fn boot() -> (u16, ulo::ShutdownHandle) {
        use ulo::UloFactory;

        let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
        let adapter = ulo_grpc::GrpcAdapter::new(addr);
        let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
        let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();
        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            let mut app = UloFactory::create(InlineGrpcModule).await.unwrap();
            app.use_grpc_adapter(adapter).unwrap();
            let bound = app.bind().await.unwrap();
            let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
            let _ = sd_tx.send(app.shutdown_handle());
            app.run().await;
        });
        tokio::task::spawn_local(async move { local.await });
        (port_rx.await.unwrap(), sd_rx.await.unwrap())
    }

    pub async fn connect(port: u16) -> pb::orders_client::OrdersClient<tonic::transport::Channel> {
        pb::orders_client::OrdersClient::new(
            tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
                .unwrap()
                .connect()
                .await
                .expect("gRPC connect should succeed"),
        )
    }

    pub fn clear() {
        SEEN.lock().unwrap().clear();
    }
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_grpc_service_s_inline_guard_and_interceptor_run() {
    grpc::clear();
    let (port, shutdown) = grpc::boot().await;
    let mut client = grpc::connect(port).await;

    client
        .create(grpc::pb::CreateOrderRequest {
            item: "book".into(),
            qty: 1,
        })
        .await
        .expect("the call must be served");
    shutdown.shutdown();

    assert_eq!(
        seen(),
        vec!["service:guard", "service:interceptor", "handler"]
    );
}

#[serial]
#[tokio_localset_test::localset_test]
async fn a_grpc_method_s_inline_guard_rejects_the_call() {
    grpc::clear();
    let (port, shutdown) = grpc::boot().await;
    let mut client = grpc::connect(port).await;

    let outcome = client
        .bulk_create(futures_util::stream::empty::<grpc::pb::CreateOrderRequest>())
        .await;
    shutdown.shutdown();

    let status = outcome.expect_err("the guard must refuse the call");
    assert!(
        !seen().contains(&"handler:denied".to_string()),
        "the handler must not run: {:?}",
        seen()
    );
    assert!(seen().contains(&"deny".to_string()), "seen: {:?}", seen());
    let _ = status;
}
