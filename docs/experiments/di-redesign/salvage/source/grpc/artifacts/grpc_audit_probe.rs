//! Audit probe for the gRPC pass, run against master `654c7183` on 2026-09-23.
//! Records what gRPC does today; it asserts nothing, so it cannot fail.
//!
//! To run it: drop this file into `integration-tests/tests/integration/`, add
//! `mod grpc_audit_probe;` to that directory's `main.rs`, then
//!
//! ```text
//! cargo test -p integration-tests --test integration probe_ -- --nocapture --test-threads=1
//! ```
//!
//! It needs no broker and no Docker — a tonic client against a ulo server
//! in-process is the whole harness. What each probe answered is in
//! `GRPC_AUDIT.md` §3.

#![allow(dead_code)]

use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::Stream;
use serial_test::serial;
use ulo::UloFactory;
use ulo::context::ExecutionContext;
use ulo::extract::Payload;
use ulo::grpc::GrpcContext;
use ulo::grpc::extract::Inbound;
use ulo_macros::{controller, grpc_methods, injectable, module, new, use_guards};

use crate::common::NotServed;

mod probe_pb {
    tonic::include_proto!("ulo_test.orders");
}

use probe_pb::orders_client::OrdersClient;
use probe_pb::orders_server::{Orders, OrdersServer};

type EventStream = Pin<Box<dyn Stream<Item = Result<probe_pb::ProgressEvent, NotServed>> + Send>>;

static HANDLER_FINISHED: AtomicBool = AtomicBool::new(false);
static TOKEN_FIRED: AtomicBool = AtomicBool::new(false);
static HANDLER_ENTERED: AtomicBool = AtomicBool::new(false);
static SEEN_HEADERS: Mutex<Option<std::collections::HashMap<String, String>>> = Mutex::new(None);
static INFLIGHT: AtomicUsize = AtomicUsize::new(0);
static FUTURE_DROPPED: AtomicBool = AtomicBool::new(false);
static GUARD_ARMED: AtomicBool = AtomicBool::new(false);
static DETACHED_RAN_ON: AtomicUsize = AtomicUsize::new(0);

/// Fires on drop unless disarmed, which is how the probe tells a dropped
/// handler future from one that ran to completion.
struct DropWatch(bool);

impl Drop for DropWatch {
    fn drop(&mut self) {
        if !self.0 {
            FUTURE_DROPPED.store(true, Ordering::SeqCst);
        }
    }
}

#[injectable]
pub struct DenyingGuard {}
impl DenyingGuard {}

#[ulo::async_trait]
impl ulo::enhancer::Guard<ulo::grpc::GrpcContext> for DenyingGuard {
    /// Gated so one service serves every probe: only PROBE 8 arms it.
    async fn can_activate(&self, _ctx: &ulo::grpc::GrpcContext) -> bool {
        !GUARD_ARMED.load(Ordering::SeqCst)
    }
}

#[controller]
pub struct ProbeService {}

impl ProbeService {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
}

#[grpc_methods(probe_pb::orders_server::Orders)]
#[use_guards(DenyingGuard)]
impl ProbeService {
    /// Sleeps well past any deadline the caller sets, watching its own token.
    #[grpc_method]
    async fn create(
        &self,
        Payload(req): Payload<probe_pb::CreateOrderRequest>,
        ctx: &GrpcContext,
    ) -> Result<probe_pb::CreateOrderResponse, NotServed> {
        HANDLER_ENTERED.store(true, Ordering::SeqCst);
        *SEEN_HEADERS.lock().unwrap() = Some(ctx.headers().clone());
        INFLIGHT.fetch_add(1, Ordering::SeqCst);
        let mut watch = DropWatch(false);

        // Detached work keyed on the token: the thing a dropped future does
        // not stop and the token is supposed to.
        let detached = ctx.cancellation().clone();
        tokio::spawn(async move {
            tokio::select! {
                _ = detached.cancelled() => { DETACHED_RAN_ON.store(1, Ordering::SeqCst); }
                _ = tokio::time::sleep(Duration::from_millis(1200)) => {
                    DETACHED_RAN_ON.store(2, Ordering::SeqCst);
                }
            }
        });

        let sleep_ms: u64 = req.qty as u64;
        if sleep_ms > 0 {
            tokio::select! {
                _ = ctx.cancellation().cancelled() => {
                    TOKEN_FIRED.store(true, Ordering::SeqCst);
                }
                _ = tokio::time::sleep(Duration::from_millis(sleep_ms)) => {}
            }
        }
        INFLIGHT.fetch_sub(1, Ordering::SeqCst);
        watch.0 = true;
        HANDLER_FINISHED.store(true, Ordering::SeqCst);
        Ok(probe_pb::CreateOrderResponse {
            id: 1,
            status: "ok".to_string(),
        })
    }

    #[grpc_stream]
    async fn watch_progress(
        &self,
        Payload(_req): Payload<probe_pb::WatchRequest>,
        ctx: &GrpcContext,
    ) -> Result<
        impl Stream<Item = Result<probe_pb::ProgressEvent, NotServed>> + Send + 'static,
        NotServed,
    > {
        HANDLER_ENTERED.store(true, Ordering::SeqCst);
        let token = ctx.cancellation().clone();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            for _ in 0..400 {
                let _ = tx.send(Ok(probe_pb::ProgressEvent {
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
            HANDLER_FINISHED.store(true, Ordering::SeqCst);
        });
        Ok(tokio_stream::wrappers::UnboundedReceiverStream::new(rx))
    }

    #[grpc_method]
    async fn bulk_create(
        &self,
        _inbound: Inbound<probe_pb::CreateOrderRequest>,
    ) -> Result<probe_pb::BulkCreateResponse, NotServed> {
        Err(NotServed)
    }

    #[grpc_stream]
    async fn chat(
        &self,
        _inbound: Inbound<probe_pb::ChatMessage>,
    ) -> Result<
        impl Stream<Item = Result<probe_pb::ChatMessage, NotServed>> + Send + 'static,
        NotServed,
    > {
        Ok(futures_util::stream::empty())
    }
}

#[module(controllers: [ProbeService], providers: [DenyingGuard])]
impl ProbeModule {}

async fn boot(max_inflight: Option<usize>) -> (u16, ulo::ShutdownHandle) {
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut adapter = ulo_grpc::GrpcAdapter::new(addr);
    if let Some(n) = max_inflight {
        adapter = adapter.with_max_inflight(n);
    }
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<ulo::ShutdownHandle>();
    tokio::spawn(async move {
        let mut app = UloFactory::create(ProbeModule).await.unwrap();
        app.use_grpc_adapter(adapter).unwrap();
        let bound = app.bind().await.unwrap();
        let port = bound.grpc.expect("grpc must bind").port();
        let _ = port_tx.send(port);
        let _ = shutdown_tx.send(app.shutdown_handle());
        app.run().await;
    });
    (port_rx.await.unwrap(), shutdown_rx.await.unwrap())
}

async fn connect(port: u16) -> OrdersClient<tonic::transport::Channel> {
    OrdersClient::new(
        tonic::transport::Endpoint::from_shared(format!("http://127.0.0.1:{}", port))
            .unwrap()
            .connect()
            .await
            .expect("connect"),
    )
}

fn order(sleep_ms: u32) -> probe_pb::CreateOrderRequest {
    probe_pb::CreateOrderRequest {
        item: "probe".to_string(),
        qty: sleep_ms,
    }
}

fn reset() {
    HANDLER_FINISHED.store(false, Ordering::SeqCst);
    TOKEN_FIRED.store(false, Ordering::SeqCst);
    HANDLER_ENTERED.store(false, Ordering::SeqCst);
    FUTURE_DROPPED.store(false, Ordering::SeqCst);
    GUARD_ARMED.store(false, Ordering::SeqCst);
    DETACHED_RAN_ON.store(0, Ordering::SeqCst);
    *SEEN_HEADERS.lock().unwrap() = None;
}

/// PROBE 1 — the caller's `grpc-timeout` expires while the handler runs.
#[serial]
#[tokio::test]
async fn probe_deadline_expiry() {
    reset();
    let (port, shutdown) = boot(None).await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(order(1500));
    request.set_timeout(Duration::from_millis(300));
    let outcome = client.create(request).await;

    eprintln!(
        "[PROBE 1] client saw: {:?}",
        outcome.as_ref().map(|_| "Ok").map_err(|s| (s.code(), s.message().to_string()))
    );
    // Give the handler room to run past the deadline.
    tokio::time::sleep(Duration::from_millis(1600)).await;
    eprintln!(
        "[PROBE 1] handler entered={} finished={} token_fired={} future_dropped={} detached={}",
        HANDLER_ENTERED.load(Ordering::SeqCst),
        HANDLER_FINISHED.load(Ordering::SeqCst),
        TOKEN_FIRED.load(Ordering::SeqCst),
        FUTURE_DROPPED.load(Ordering::SeqCst),
        match DETACHED_RAN_ON.load(Ordering::SeqCst) {
            1 => "cancelled",
            2 => "ran to completion",
            _ => "still running",
        },
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}

/// PROBE 2 — the caller abandons a unary call in flight.
#[serial]
#[tokio::test]
async fn probe_unary_abandoned() {
    reset();
    let (port, shutdown) = boot(None).await;
    let mut client = connect(port).await;

    let call = client.create(order(1500));
    let abandoned = tokio::time::timeout(Duration::from_millis(250), call).await;
    drop(abandoned);
    drop(client);

    tokio::time::sleep(Duration::from_millis(1600)).await;
    eprintln!(
        "[PROBE 2] handler entered={} finished={} token_fired={} future_dropped={} detached={}",
        HANDLER_ENTERED.load(Ordering::SeqCst),
        HANDLER_FINISHED.load(Ordering::SeqCst),
        TOKEN_FIRED.load(Ordering::SeqCst),
        FUTURE_DROPPED.load(Ordering::SeqCst),
        match DETACHED_RAN_ON.load(Ordering::SeqCst) {
            1 => "cancelled",
            2 => "ran to completion",
            _ => "still running",
        },
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}

/// PROBE 3 — a repeated ASCII key and a `-bin` key on the way in.
#[serial]
#[tokio::test]
async fn probe_metadata_shapes() {
    reset();
    let (port, shutdown) = boot(None).await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(order(0));
    let md = request.metadata_mut();
    md.append("x-tag", "first".parse().unwrap());
    md.append("x-tag", "second".parse().unwrap());
    md.insert_bin(
        "x-trace-bin",
        tonic::metadata::MetadataValue::from_bytes(&[0xDE, 0xAD, 0xBE, 0xEF]),
    );
    md.insert("x-plain", "kept".parse().unwrap());
    client.create(request).await.expect("call must succeed");

    let seen = SEEN_HEADERS.lock().unwrap().clone().expect("handler ran");
    let mut keys: Vec<_> = seen.keys().cloned().collect();
    keys.sort();
    eprintln!("[PROBE 3] keys the handler sees: {keys:?}");
    eprintln!("[PROBE 3] x-tag = {:?}", seen.get("x-tag"));
    eprintln!("[PROBE 3] x-trace-bin = {:?}", seen.get("x-trace-bin"));

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}

/// PROBE 4 — the global cap sheds load.
#[serial]
#[tokio::test]
async fn probe_load_shed() {
    reset();
    let (port, shutdown) = boot(Some(1)).await;
    let mut a = connect(port).await;
    let mut b = connect(port).await;

    let first = tokio::spawn(async move { a.create(order(800)).await.map(|_| ()) });
    tokio::time::sleep(Duration::from_millis(200)).await;
    let second = b.create(order(0)).await;

    eprintln!(
        "[PROBE 4] second call: {:?}",
        second.as_ref().map(|_| "Ok").map_err(|s| (s.code(), s.message().to_string()))
    );
    let _ = first.await;

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}

/// PROBE 5 — `GrpcContext::new` is public and parses `grpc-timeout` eagerly.
/// A value whose last byte is not a char boundary reaches `str::split_at`.
#[test]
fn probe_grpc_timeout_non_ascii() {
    let mut headers = std::collections::HashMap::new();
    headers.insert("grpc-timeout".to_string(), "5\u{20AC}".to_string());
    let outcome = std::panic::catch_unwind(|| {
        let ctx = GrpcContext::new("pkg.Svc/Method", headers, None, None);
        ctx.deadline()
    });
    eprintln!("[PROBE 5] non-ASCII grpc-timeout: {:?}", outcome.is_err());
    if let Err(e) = &outcome {
        let msg = e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
        eprintln!("[PROBE 5] payload: {msg:?}");
    }
}

/// PROBE 6 — an overlong but well-formed timeout.
#[test]
fn probe_grpc_timeout_overflow() {
    for value in ["99999999H", "00000005S", "0S"] {
        let mut headers = std::collections::HashMap::new();
        headers.insert("grpc-timeout".to_string(), value.to_string());
        let outcome = std::panic::catch_unwind(|| {
            GrpcContext::new("pkg.Svc/Method", headers, None, None).deadline()
        });
        eprintln!("[PROBE 6] {value} -> panicked={}", outcome.is_err());
    }
}

/// PROBE 7 — a streaming reply whose caller's deadline expires. The unary
/// counterpart is PROBE 1; this is the same trigger against the path
/// ADR-0033 wired.
#[serial]
#[tokio::test]
async fn probe_streaming_deadline() {
    reset();
    let (port, shutdown) = boot(None).await;
    let mut client = connect(port).await;

    let mut request = tonic::Request::new(probe_pb::WatchRequest { id: 1 });
    request.set_timeout(Duration::from_millis(300));
    let outcome = client.watch_progress(request).await;
    match outcome {
        Ok(response) => {
            let mut stream = response.into_inner();
            let mut count = 0usize;
            loop {
                match tokio::time::timeout(
                    Duration::from_millis(900),
                    futures_util::StreamExt::next(&mut stream),
                )
                .await
                {
                    Ok(Some(Ok(_))) => count += 1,
                    Ok(Some(Err(e))) => {
                        eprintln!("[PROBE 7] stream ended: {:?} {}", e.code(), e.message());
                        break;
                    }
                    Ok(None) => break,
                    Err(_) => {
                        eprintln!("[PROBE 7] stream stalled");
                        break;
                    }
                }
            }
            eprintln!("[PROBE 7] items received: {count}");
        }
        Err(e) => eprintln!("[PROBE 7] call failed: {:?} {}", e.code(), e.message()),
    }

    tokio::time::sleep(Duration::from_millis(400)).await;
    eprintln!(
        "[PROBE 7] handler entered={} token_fired={}",
        HANDLER_ENTERED.load(Ordering::SeqCst),
        TOKEN_FIRED.load(Ordering::SeqCst),
    );

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}

/// PROBE 8 — what a refused caller is told: the code, and the message.
#[serial]
#[tokio::test]
async fn probe_guard_refusal_on_the_wire() {
    reset();
    GUARD_ARMED.store(true, Ordering::SeqCst);
    let (port, shutdown) = boot(None).await;
    let mut client = connect(port).await;

    let err = client
        .create(order(0))
        .await
        .expect_err("the guard refuses everything");
    eprintln!(
        "[PROBE 8] code={:?} message={:?}",
        err.code(),
        err.message()
    );
    eprintln!("[PROBE 8] handler entered={}", HANDLER_ENTERED.load(Ordering::SeqCst));

    shutdown.shutdown();
    let _ = tokio::time::timeout(Duration::from_secs(3), shutdown.completed()).await;
}
