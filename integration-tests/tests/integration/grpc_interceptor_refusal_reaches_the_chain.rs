//! An interceptor that refuses a gRPC call is offered to the error chain, as one that panics was.
//!
//! A refusal and a panic leave an interceptor the same way — the `Err` side of its answer — and the
//! chain runs once above them, so a `#[catch]` handler is offered either. The case is worth pinning
//! because the two travelled differently while the chain sat below the interceptors: the panic was
//! routed to it and the deliberate `Err` went straight to the wire.

#![allow(dead_code)]

use crate::common::NotServed;
use serial_test::serial;
use ulo::UloFactory;
use ulo::enhancer::{ChainError, ErrorHandler, Interceptor, InterceptorNext};
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo::grpc::{GrpcCode, GrpcContext, GrpcHandlerResult, GrpcStatus};
use ulo::{async_trait, injectable, module};
use ulo_macros::{controller, grpc_methods, new, use_error_handlers, use_interceptors};

mod refusal_pb {
    tonic::include_proto!("ulo_test.orders");
}

use refusal_pb::orders_client::OrdersClient;
use refusal_pb::orders_server::{Orders, OrdersServer};

/// Refuses without calling `next`, which is the shape an interceptor uses to answer for a handler
/// it declines to run — an expired token, a cache miss it cannot fill, a closed window.
#[injectable]
pub struct RefusingInterceptor {}

#[async_trait]
impl Interceptor<GrpcContext, GrpcHandlerResult> for RefusingInterceptor {
    async fn intercept(
        &self,
        _ctx: &GrpcContext,
        _next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
    ) -> GrpcHandlerResult {
        Err(GrpcStatus::new(
            GrpcCode::FailedPrecondition,
            "refused before the handler",
        ))
    }
}

/// Claims whatever reaches it, so the test asserts the refusal *arrived* rather than asserting
/// which type it arrived as.
#[injectable]
pub struct ClaimsAnything {}

#[async_trait]
impl ErrorHandler<GrpcContext, GrpcHandlerResult> for ClaimsAnything {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcHandlerResult> {
        Some(Err(GrpcStatus::new(
            GrpcCode::ResourceExhausted,
            "claimed by the chain",
        )))
    }
}

#[controller]
pub struct RefusedGrpcService {}

impl RefusedGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(refusal_pb::orders_server::Orders)]
impl RefusedGrpcService {
    /// Refused with a handler registered to claim it.
    #[grpc_method]
    #[use_interceptors(RefusingInterceptor)]
    #[use_error_handlers(ClaimsAnything)]
    async fn create(
        &self,
        Payload(_req): Payload<refusal_pb::CreateOrderRequest>,
    ) -> Result<refusal_pb::CreateOrderResponse, NotServed> {
        Err(NotServed)
    }

    /// Refused with nothing registered to claim it: the interceptor's own status is the answer.
    #[grpc_method]
    #[use_interceptors(RefusingInterceptor)]
    async fn bulk_create(
        &self,
        _inbound: Inbound<refusal_pb::CreateOrderRequest>,
    ) -> Result<refusal_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<refusal_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<refusal_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<refusal_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<refusal_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(
    controllers: [RefusedGrpcService],
    providers: [RefusingInterceptor, ClaimsAnything]
)]
impl RefusedGrpcModule {}

async fn boot() -> u16 {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = ulo_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = UloFactory::new()
            .create_with(RefusedGrpcModule)
            .await
            .unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        app.run().await;
    });
    port_rx.await.unwrap()
}

async fn client(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    )
}

#[serial]
#[tokio::test]
async fn a_refusal_is_offered_to_the_chain() {
    let mut client = client(boot().await).await;

    let err = client
        .create(refusal_pb::CreateOrderRequest {
            item: "unobtainium".to_string(),
            qty: 1,
        })
        .await
        .expect_err("the interceptor refused the call");

    // The handler's status, not the interceptor's: the refusal reached the chain and was claimed.
    assert_eq!(err.code(), tonic::Code::ResourceExhausted);
    assert_eq!(err.message(), "claimed by the chain");
}

#[serial]
#[tokio::test]
async fn an_unclaimed_refusal_keeps_the_status_the_interceptor_chose() {
    let mut client = client(boot().await).await;

    let err = client
        .bulk_create(futures_util::stream::iter(vec![
            refusal_pb::CreateOrderRequest {
                item: "unobtainium".to_string(),
                qty: 1,
            },
        ]))
        .await
        .expect_err("the interceptor refused the call");

    assert_eq!(err.code(), tonic::Code::FailedPrecondition);
    assert_eq!(err.message(), "refused before the handler");
}
