//! The caller's `grpc-timeout` fires the execution's cancellation token when it passes, for as
//! long as the execution lasts; so does a call dropped before it answers, by its caller or at its
//! deadline.
//!
//! tonic races the call against the deadline until the handler returns its response, which for
//! a streaming method is the moment it has a stream; the body is then outside the race. ulo fires
//! the token at the deadline, and a producer feeding a stream past it sees it fire and stops. A
//! call that has already answered is over, and its token does not fire later.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::common::NotServed;
use futures_util::Stream;
use serial_test::serial;
use ulo::UloFactory;
use ulo::async_trait;
use ulo::context::ExecutionContext;
use ulo::enhancer::Guard;
use ulo::extract::Payload;
use ulo::grpc::GrpcContext;
use ulo::grpc::extract::Inbound;
use ulo_macros::{controller, grpc_methods, module, new, use_guards};

mod deadline_pb {
    tonic::include_proto!("ulo_test.orders");
}

use deadline_pb::orders_client::OrdersClient;
use deadline_pb::orders_server::{Orders, OrdersServer};

static TOKEN_FIRED: AtomicBool = AtomicBool::new(false);
/// 0 while running, 1 when the token stopped it, 2 when it ran to completion.
static DETACHED_RAN_ON: AtomicUsize = AtomicUsize::new(0);

fn reset() {
    TOKEN_FIRED.store(false, Ordering::SeqCst);
    DETACHED_RAN_ON.store(0, Ordering::SeqCst);
}

static EXECUTION_FREED: AtomicBool = AtomicBool::new(false);
/// Set if the slow call's handler ran, which would mean tonic did not drop it at the deadline.
static CHAT_RAN: AtomicBool = AtomicBool::new(false);

/// Put in an execution's extensions; records that the execution was freed.
struct RecordsItsDrop;

impl Drop for RecordsItsDrop {
    fn drop(&mut self) {
        EXECUTION_FREED.store(true, Ordering::SeqCst);
    }
}

/// Marks the execution, then takes longer than any deadline the test sets, so tonic drops the
/// call before any extractor takes the request.
struct SlowGuard;

#[async_trait]
impl Guard<GrpcContext> for SlowGuard {
    async fn can_activate(&self, ctx: &GrpcContext) -> bool {
        ctx.extensions().insert(RecordsItsDrop);
        tokio::time::sleep(Duration::from_millis(500)).await;
        true
    }
}

/// Refuses every call, after detaching work on a clone of the token that records whether the
/// token fired within 1.2 s. The handler never runs, so nothing takes the call's request.
struct RefusingGuard;

#[async_trait]
impl Guard<GrpcContext> for RefusingGuard {
    async fn can_activate(&self, ctx: &GrpcContext) -> bool {
        let detached = ctx.cancellation().clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = detached.cancelled() => {
                    TOKEN_FIRED.store(true, Ordering::SeqCst);
                    DETACHED_RAN_ON.store(1, Ordering::SeqCst)
                }
                _ = tokio::time::sleep(Duration::from_millis(1200)) => {
                    DETACHED_RAN_ON.store(2, Ordering::SeqCst)
                }
            }
        });
        false
    }
}

#[controller]
pub struct DeadlineService {}

impl DeadlineService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(deadline_pb::orders_server::Orders)]
impl DeadlineService {
    /// Sleeps for `qty` milliseconds, and spawns work keyed on a clone of the token that records
    /// whether the token fired within 1.2 s.
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<deadline_pb::CreateOrderRequest>,
        ctx: &GrpcContext,
    ) -> Result<deadline_pb::CreateOrderResponse, NotServed> {
        let detached = ctx.cancellation().clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = detached.cancelled() => {
                    TOKEN_FIRED.store(true, Ordering::SeqCst);
                    DETACHED_RAN_ON.store(1, Ordering::SeqCst)
                }
                _ = tokio::time::sleep(Duration::from_millis(1200)) => {
                    DETACHED_RAN_ON.store(2, Ordering::SeqCst)
                }
            }
        });

        tokio::time::sleep(Duration::from_millis(u64::from(req.qty))).await;
        Ok(deadline_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        })
    }

    /// Four hundred items, 25 ms apart, from a producer that watches the token.
    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<deadline_pb::WatchRequest>,
        ctx: &GrpcContext,
    ) -> Result<
        impl Stream<Item = Result<deadline_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        let token = ctx.cancellation().clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for _ in 0..400 {
                let _ = tx.send(Ok(deadline_pb::ProgressEvent {
                    id: 1,
                    status: "tick".to_string(),
                }));
                tokio::select! {
                    _ = token.cancelled() => {
                        TOKEN_FIRED.store(true, Ordering::SeqCst);
                        return;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(25)) => {}
                }
            }
        });
        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    #[grpc_method]
    #[use_guards(RefusingGuard {})]
    async fn bulk_create(
        &self,
        _inbound: Inbound<deadline_pb::CreateOrderRequest>,
    ) -> Result<deadline_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    #[use_guards(SlowGuard {})]
    async fn chat(
        &self,
        _inbound: Inbound<deadline_pb::ChatMessage>,
    ) -> Result<
        impl Stream<Item = Result<deadline_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        CHAT_RAN.store(true, Ordering::SeqCst);
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [DeadlineService])]
impl DeadlineModule {}

async fn boot() -> (u16, ulo::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let adapter = ulo_grpc::GrpcAdapter::new(addr);
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(DeadlineModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.grpc.expect("grpc must bind").port());
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{port}"))
            .unwrap()
            .connect()
            .await
            .expect("gRPC connect should succeed"),
    )
}

async fn stop(shutdown: ulo::ShutdownHandle) {
    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}

/// A call that answers before its deadline is over: its token does not fire when the deadline
/// later passes, and work it detached runs to completion.
#[serial]
#[tokio::test]
async fn a_call_that_answered_before_its_deadline_does_not_fire_later() {
    reset();
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(deadline_pb::CreateOrderRequest {
        item: "keyboard".to_string(),
        qty: 10,
    });
    request.set_timeout(Duration::from_millis(300));
    client
        .create(request)
        .await
        .expect("a 10 ms call answers inside a 300 ms deadline");

    // Past the deadline, and past the detached work's own 1.2 s.
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(
        !TOKEN_FIRED.load(Ordering::SeqCst),
        "the token fired at the deadline of a call that had already answered"
    );
    assert_eq!(DETACHED_RAN_ON.load(Ordering::SeqCst), 2);
    stop(shutdown).await;
}

/// A call a guard refused is over too, though its handler never took the request.
#[serial]
#[tokio::test]
async fn a_refused_call_does_not_fire_its_token_at_the_deadline() {
    reset();
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut request =
        tonic::Request::new(futures_util::stream::empty::<deadline_pb::CreateOrderRequest>());
    request.set_timeout(Duration::from_millis(300));
    let status = client
        .bulk_create(request)
        .await
        .expect_err("the guard refuses the call");
    assert_eq!(status.code(), tonic::Code::PermissionDenied);

    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(
        !TOKEN_FIRED.load(Ordering::SeqCst),
        "the token fired at the deadline of a call a guard had already refused"
    );
    assert_eq!(DETACHED_RAN_ON.load(Ordering::SeqCst), 2);
    stop(shutdown).await;
}

/// A call tonic drops at its deadline, before anything took its request, is freed.
#[serial]
#[tokio::test]
async fn a_call_dropped_before_its_request_was_taken_is_freed() {
    reset();
    EXECUTION_FREED.store(false, Ordering::SeqCst);
    CHAT_RAN.store(false, Ordering::SeqCst);
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut request =
        tonic::Request::new(futures_util::stream::pending::<deadline_pb::ChatMessage>());
    request.set_timeout(Duration::from_millis(300));
    let _ = client.chat(request).await;

    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        !CHAT_RAN.load(Ordering::SeqCst),
        "the handler ran, so tonic did not drop the call at its deadline"
    );
    assert!(
        EXECUTION_FREED.load(Ordering::SeqCst),
        "a call dropped at its deadline before its request was taken was never freed"
    );
    stop(shutdown).await;
}

/// A unary call past its deadline is dropped by tonic, and the work its handler detached sees the
/// token fire and stops.
#[serial]
#[tokio::test]
async fn detached_work_stops_at_a_unary_calls_deadline() {
    reset();
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(deadline_pb::CreateOrderRequest {
        item: "keyboard".to_string(),
        qty: 1500,
    });
    request.set_timeout(Duration::from_millis(300));
    let outcome = client.create(request).await;
    assert!(
        outcome.is_err(),
        "the caller's 300 ms deadline must end a 1.5 s call"
    );

    // Past the detached work's own 1.2 s, so a token that never fired shows as a completed run.
    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(
        TOKEN_FIRED.load(Ordering::SeqCst),
        "the token did not fire when the deadline passed"
    );
    assert_eq!(
        DETACHED_RAN_ON.load(Ordering::SeqCst),
        1,
        "detached work holding a clone of the token ran on past the deadline"
    );
    stop(shutdown).await;
}

/// A unary call the caller drops mid-flight, with no deadline, fires the token too.
#[serial]
#[tokio::test]
async fn detached_work_stops_when_the_caller_abandons_a_unary_call() {
    reset();
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let call = tokio::spawn(async move {
        client
            .create(tonic::Request::new(deadline_pb::CreateOrderRequest {
                item: "keyboard".to_string(),
                qty: 1500,
            }))
            .await
    });
    tokio::time::sleep(Duration::from_millis(250)).await;
    call.abort();
    let _ = call.await;

    tokio::time::sleep(Duration::from_millis(1400)).await;
    assert!(
        TOKEN_FIRED.load(Ordering::SeqCst),
        "the token did not fire when the caller abandoned the call"
    );
    assert_eq!(
        DETACHED_RAN_ON.load(Ordering::SeqCst),
        1,
        "detached work holding a clone of the token ran on after the caller left"
    );
    stop(shutdown).await;
}

/// A streaming reply is cancelled when the caller's deadline passes, which tonic's own race
/// does not reach: it ends when the handler returns its stream.
#[serial]
#[tokio::test]
async fn a_streaming_reply_stops_at_its_deadline() {
    reset();
    let (port, shutdown) = boot().await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(deadline_pb::WatchRequest { id: 1 });
    request.set_timeout(Duration::from_millis(300));
    let mut stream = client
        .watch_progress(request)
        .await
        .expect("the stream opens before the deadline")
        .into_inner();

    let mut count = 0usize;
    loop {
        match tokio::time::timeout(
            Duration::from_millis(900),
            futures_util::StreamExt::next(&mut stream),
        )
        .await
        {
            Ok(Some(Ok(_))) => count += 1,
            Ok(Some(Err(_))) | Ok(None) | Err(_) => break,
        }
    }

    assert!(
        count < 400,
        "the producer delivered all {count} items past a 300 ms deadline"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        TOKEN_FIRED.load(Ordering::SeqCst),
        "the producer never saw the token fire"
    );
    stop(shutdown).await;
}
