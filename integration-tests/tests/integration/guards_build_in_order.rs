//! A guard that refuses is the last thing built: no later guard and no interceptor is
//! constructed for a call it turned away.
//!
//! A `GuardEntry::Factory` entry is an execution-scoped provider's own resolution, with its
//! dependencies built with it, so a guard built before an earlier guard refuses is the work a
//! refusal exists to avoid. Pinned on HTTP, whose call-site shape RPC and WebSocket share, and on
//! gRPC, which walks its guards from its own pipeline. The admitted call is the control: the same
//! providers record their builds when they run.

#![allow(dead_code)]

use std::sync::Mutex;

use serial_test::serial;
use ulo::async_trait;
use ulo::enhancer::{Guard, Interceptor, InterceptorNext};
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo::grpc::{GrpcContext, GrpcHandlerResult};
use ulo::http::{Body, HttpContext, HttpHandlerResult};
use ulo::{controller, get, injectable, module, routes, use_guards, use_interceptors};
use ulo_macros::{grpc_methods, new};

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

/// Refuses every call, and records that it was asked.
#[injectable]
pub struct Refusing {}
impl Refusing {}

/// Admits every call, and records that it was asked.
#[injectable]
pub struct Admitting {}
impl Admitting {}

/// Execution-scoped, so it is built per call, and records the build.
#[injectable(scope = "execution")]
pub struct BuiltGuard {}

impl BuiltGuard {
    #[new]
    pub fn new() -> Self {
        record("built:guard");
        Self {}
    }
}

#[injectable(scope = "execution")]
pub struct BuiltInterceptor {}

impl BuiltInterceptor {
    #[new]
    pub fn new() -> Self {
        record("built:interceptor");
        Self {}
    }
}

macro_rules! impl_guards {
    ($ctx:ty) => {
        #[async_trait]
        impl Guard<$ctx> for Refusing {
            async fn can_activate(&self, _ctx: &$ctx) -> bool {
                record("ran:refusing");
                false
            }
        }

        #[async_trait]
        impl Guard<$ctx> for Admitting {
            async fn can_activate(&self, _ctx: &$ctx) -> bool {
                record("ran:admitting");
                true
            }
        }

        #[async_trait]
        impl Guard<$ctx> for BuiltGuard {
            async fn can_activate(&self, _ctx: &$ctx) -> bool {
                record("ran:guard");
                true
            }
        }
    };
}
impl_guards!(HttpContext);
impl_guards!(GrpcContext);

#[async_trait]
impl Interceptor<HttpContext, HttpHandlerResult> for BuiltInterceptor {
    async fn intercept(
        &self,
        ctx: &HttpContext,
        next: Box<dyn InterceptorNext<HttpContext, HttpHandlerResult>>,
    ) -> HttpHandlerResult {
        record("ran:interceptor");
        next.run(ctx).await
    }
}

#[async_trait]
impl Interceptor<GrpcContext, GrpcHandlerResult> for BuiltInterceptor {
    async fn intercept(
        &self,
        ctx: &GrpcContext,
        next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
    ) -> GrpcHandlerResult {
        record("ran:interceptor");
        next.run(ctx).await
    }
}

// ── HTTP ─────────────────────────────────────────────────────────────────────

#[controller("/refused")]
pub struct RefusedController {}

#[routes]
#[use_guards(Refusing, BuiltGuard)]
#[use_interceptors(BuiltInterceptor)]
impl RefusedController {
    #[get("/")]
    fn get(&self) -> Body {
        record("handler");
        Body::text("ok".to_string())
    }
}

#[controller("/admitted")]
pub struct AdmittedController {}

#[routes]
#[use_guards(Admitting, BuiltGuard)]
#[use_interceptors(BuiltInterceptor)]
impl AdmittedController {
    #[get("/")]
    fn get(&self) -> Body {
        record("handler");
        Body::text("ok".to_string())
    }
}

#[module(
    controllers: [RefusedController, AdmittedController],
    providers: [Refusing, Admitting, BuiltGuard, BuiltInterceptor],
)]
impl HttpBuildOrderModule {}

#[serial]
#[tokio::test]
async fn http_a_refused_call_builds_nothing_below_the_refusing_guard() {
    clear();
    let server = TestServer::start(HttpBuildOrderModule).await;

    let response = server
        .client()
        .get(server.url("/refused/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);

    assert_eq!(seen(), vec!["ran:refusing"]);
}

#[serial]
#[tokio::test]
async fn http_an_admitted_call_builds_each_thing_as_it_reaches_it() {
    clear();
    let server = TestServer::start(HttpBuildOrderModule).await;

    let response = server
        .client()
        .get(server.url("/admitted/"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);

    assert_eq!(
        seen(),
        vec![
            "ran:admitting",
            "built:guard",
            "ran:guard",
            "built:interceptor",
            "ran:interceptor",
            "handler"
        ],
        "the first guard is asked before the second is built, and the interceptor after both"
    );
}

// ── gRPC ─────────────────────────────────────────────────────────────────────

mod build_pb {
    tonic::include_proto!("ulo_test.orders");
}

use build_pb::orders_server::{Orders, OrdersServer};

#[controller]
pub struct RefusedService {}

#[grpc_methods(build_pb::orders_server::Orders)]
#[use_guards(Refusing, BuiltGuard)]
#[use_interceptors(BuiltInterceptor)]
impl RefusedService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[grpc_method]
    async fn create(
        &self,
        Payload(_req): Payload<build_pb::CreateOrderRequest>,
    ) -> Result<build_pb::CreateOrderResponse, NotServed> {
        record("handler");
        Ok(build_pb::CreateOrderResponse {
            id: 1,
            status: "created".to_string(),
        })
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<build_pb::CreateOrderRequest>,
    ) -> Result<build_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<build_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<build_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<build_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<build_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(
    controllers: [RefusedService],
    providers: [Refusing, Admitting, BuiltGuard, BuiltInterceptor],
)]
impl GrpcBuildOrderModule {}

#[serial]
#[tokio::test]
async fn grpc_a_refused_call_builds_nothing_below_the_refusing_guard() {
    clear();

    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = ulo_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();
    tokio::spawn(async move {
        let mut app = ulo::UloFactory::create(GrpcBuildOrderModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        let _ = sd_tx.send(app.shutdown_handle());
        app.run().await;
    });
    let port = port_rx.await.unwrap();
    let shutdown = sd_rx.await.unwrap();

    let mut client = build_pb::orders_client::OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    );
    let outcome = client
        .create(build_pb::CreateOrderRequest {
            item: "book".into(),
            qty: 1,
        })
        .await;
    shutdown.shutdown();

    let status = outcome.expect_err("the guard must refuse the call");
    assert_eq!(status.code(), tonic::Code::PermissionDenied);
    assert_eq!(seen(), vec!["ran:refusing"]);
}
