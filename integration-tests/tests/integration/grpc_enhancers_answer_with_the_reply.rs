//! A gRPC enhancer reads the reply, replaces it, or answers with one of its own.
//!
//! The reply travels in the answer the interceptor chain and the error chain share, erased, so an
//! interceptor downcasts it to the method's own type and an error handler claiming a failure
//! recovers the call with one. Each of those is ordinary on the other three transports; here the
//! reply left the handler by a side-channel the enhancers could not see, so `next.run(ctx)`
//! answered `Ok(())` whether the handler had replied or failed.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::NotServed;
use serial_test::serial;
use ulo::UloFactory;
use ulo::enhancer::{ChainError, ErrorHandler, Interceptor, InterceptorNext};
use ulo::extract::Payload;
use ulo::grpc::extract::Inbound;
use ulo::grpc::{GrpcContext, GrpcHandlerResult, GrpcReply};
use ulo::{async_trait, controller, module};
use ulo_macros::{grpc_methods, new};

mod reply_pb {
    tonic::include_proto!("ulo_test.orders");
}

use reply_pb::orders_client::OrdersClient;
use reply_pb::orders_server::{Orders, OrdersServer};

static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn record(what: impl Into<String>) {
    SEEN.lock().unwrap().push(what.into());
}

fn seen() -> Vec<String> {
    SEEN.lock().unwrap().clone()
}

type Reply = tonic::Response<reply_pb::CreateOrderResponse>;

fn created(id: u64, status: &str) -> Reply {
    tonic::Response::new(reply_pb::CreateOrderResponse {
        id,
        status: status.to_string(),
    })
}

// ── the enhancers ──────────────────────────────────────────────────────────

/// Reads what the handler replied on the way out, leaving it alone.
struct ReadsTheReply;

#[async_trait]
impl Interceptor<GrpcContext, GrpcHandlerResult> for ReadsTheReply {
    async fn intercept(
        &self,
        ctx: &GrpcContext,
        next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
    ) -> GrpcHandlerResult {
        let answer = next.run(ctx).await;
        if let Ok(reply) = &answer {
            match reply.downcast_ref::<Reply>() {
                Some(response) => record(format!("read:{}", response.get_ref().status)),
                None => record(format!("read:other:{}", reply.carries())),
            }
        }
        answer
    }
}

/// Answers without running the handler, which is the cache-hit shape.
struct AnswersInstead;

#[async_trait]
impl Interceptor<GrpcContext, GrpcHandlerResult> for AnswersInstead {
    async fn intercept(
        &self,
        _ctx: &GrpcContext,
        _next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
    ) -> GrpcHandlerResult {
        Ok(GrpcReply::new(created(7, "cached")))
    }
}

/// Answers with a reply belonging to another method, which nothing below can render.
struct AnswersTheWrongType;

#[async_trait]
impl Interceptor<GrpcContext, GrpcHandlerResult> for AnswersTheWrongType {
    async fn intercept(
        &self,
        _ctx: &GrpcContext,
        _next: Box<dyn InterceptorNext<GrpcContext, GrpcHandlerResult>>,
    ) -> GrpcHandlerResult {
        Ok(GrpcReply::new(tonic::Response::new(
            reply_pb::ChatMessage {
                text: "not this method's reply".to_string(),
                id: 1,
            },
        )))
    }
}

/// Claims a failure and answers with a reply, which ends the call successfully.
struct RecoversWithAReply;

#[async_trait]
impl ErrorHandler<GrpcContext, GrpcHandlerResult> for RecoversWithAReply {
    async fn handle_error(
        &self,
        _error: ChainError<'_>,
        _ctx: &GrpcContext,
    ) -> Option<GrpcHandlerResult> {
        Some(Ok(GrpcReply::new(created(9, "recovered"))))
    }
}

// ── the service ────────────────────────────────────────────────────────────

#[controller]
pub struct ReplyGrpcService {}

impl ReplyGrpcService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(reply_pb::orders_server::Orders)]
impl ReplyGrpcService {
    /// Fails on `qty == 0`, so one method serves both the reply cases and the recovery case.
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<reply_pb::CreateOrderRequest>,
    ) -> Result<reply_pb::CreateOrderResponse, NotServed> {
        record("handler");
        if req.qty == 0 {
            return Err(NotServed);
        }
        Ok(reply_pb::CreateOrderResponse {
            id: 1,
            status: "created".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<reply_pb::WatchRequest>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<reply_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<reply_pb::CreateOrderRequest>,
    ) -> Result<reply_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<reply_pb::ChatMessage>,
    ) -> Result<
        impl futures_util::Stream<Item = Result<reply_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [ReplyGrpcService])]
impl ReplyGrpcModule {}

// ── harness ────────────────────────────────────────────────────────────────

async fn boot<F>(configure: F) -> (u16, ulo::ShutdownHandle)
where
    F: FnOnce(&mut UloFactory) + Send + 'static,
{
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = ulo_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = UloFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(ReplyGrpcModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    )
}

fn order(qty: u32) -> reply_pb::CreateOrderRequest {
    reply_pb::CreateOrderRequest {
        item: "keyboard".to_string(),
        qty,
    }
}

async fn stop(shutdown: ulo::ShutdownHandle) {
    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(2), shutdown.completed()).await;
}

// ── tests ──────────────────────────────────────────────────────────────────

#[serial]
#[tokio_localset_test::localset_test]
async fn an_interceptor_reads_the_reply() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_interceptors(Arc::new(ReadsTheReply));
    })
    .await;
    let mut client = connect(port).await;

    let reply = client.create(order(1)).await.expect("the handler replied");

    assert_eq!(reply.get_ref().status, "created");
    assert!(
        seen().contains(&"read:created".to_string()),
        "the interceptor read the reply the handler produced, got {:?}",
        seen()
    );
    stop(shutdown).await;
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_interceptor_answers_without_running_the_handler() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_interceptors(Arc::new(AnswersInstead));
    })
    .await;
    let mut client = connect(port).await;

    let reply = client
        .create(order(1))
        .await
        .expect("the interceptor replied");

    assert_eq!(reply.get_ref().id, 7);
    assert_eq!(reply.get_ref().status, "cached");
    assert!(
        !seen().contains(&"handler".to_string()),
        "the handler must not run when the interceptor answers for it"
    );
    stop(shutdown).await;
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_error_handler_recovers_the_call_with_a_reply() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_error_handler(Arc::new(RecoversWithAReply));
    })
    .await;
    let mut client = connect(port).await;

    // qty = 0 fails the handler; the claim turns that failure into a reply.
    let reply = client
        .create(order(0))
        .await
        .expect("the claim recovered the call");

    assert_eq!(reply.get_ref().id, 9);
    assert_eq!(reply.get_ref().status, "recovered");
    stop(shutdown).await;
}

/// A reply of the wrong type fails that call, naming both types.
///
/// One list of enhancers serves every method of a service and each method's reply is its own type,
/// so the reply travels erased and the wrapper downcasts it. Nothing below the wrapper can render
/// a value of another method's type, and this is where that is said.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_enhancer_answering_another_methods_reply_fails_the_call() {
    SEEN.lock().unwrap().clear();

    let (port, shutdown) = boot(|f| {
        f.use_global_grpc_interceptors(Arc::new(AnswersTheWrongType));
    })
    .await;
    let mut client = connect(port).await;

    let err = client
        .create(order(1))
        .await
        .expect_err("a reply of another method's type cannot be rendered");

    assert_eq!(err.code(), tonic::Code::Internal);
    assert!(
        err.message().contains("ChatMessage") && err.message().contains("CreateOrderResponse"),
        "the diagnostic names what arrived and what the method replies, got {:?}",
        err.message()
    );
    stop(shutdown).await;
}
