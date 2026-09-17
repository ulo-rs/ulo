//! The contract every RPC transport satisfies, written once.
//!
//! Seven transports speak ulo's wire grammar, and the behaviour a caller
//! depends on is the same across all of them: a request comes back, an emit
//! reaches its handler without one, headers survive the trip, a stream arrives
//! in order, abandoning a stream cancels the producer, and traffic resumes
//! after the connection breaks. Written per transport, that contract was seven
//! copies which drifted — `ulo-rpc-nats` was missing three cases outright and
//! nothing failed, because there was no suite for them to be missing from.
//!
//! Here the cases are generic functions over [`Broker`], and
//! [`conformance_suite!`] stamps one `#[tokio::test]` per case in the
//! transport's own `tests/`. A transport that cannot satisfy a case says so by
//! failing it, and a new case reaches every transport at once.
//!
//! # Implementing it for a transport
//!
//! ```ignore
//! struct RedisBroker {
//!     _container: ContainerAsync<Redis>,
//!     endpoint: String,
//! }
//!
//! impl Broker for RedisBroker {
//!     type Adapter = RedisAdapter;
//!     type Transport = RedisClientTransport;
//!
//!     async fn start() -> Self { /* testcontainer, then the endpoint */ }
//!     fn adapter(&self) -> Self::Adapter { RedisAdapter::new(&self.endpoint) }
//!     fn transport(&self) -> Self::Transport { RedisClientTransport::new(&self.endpoint) }
//!     async fn disrupt(&self) { /* whatever severs the connection */ }
//! }
//!
//! ulo_rpc_conformance::conformance_suite!(RedisBroker);
//! ```
//!
//! The service-backed half stays in each transport's crate rather than moving
//! into `integration-tests`: these need Docker, and that suite is hermetic.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;
use ulo::dispatch::Cardinality;

use futures::StreamExt;
use ulo::UloFactory;
use ulo::context::ExecutionContext;
use ulo::rpc::RpcClient;
use ulo::rpc::RpcContext;
use ulo::rpc::{RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult};
use ulo_macros::{controller, module, new, patterns};

/// How long each phase may take. A Kafka broker boots slowly and assigns
/// consumer groups before the first request is consumed, so the budgets are a
/// transport's to raise.
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    /// Waiting for the server's subscriptions to come up before the first
    /// request. Every broker subscribes asynchronously after `bind`.
    pub boot: Duration,
    /// Waiting for a one-way effect (an emit reaching its handler, a producer
    /// noticing cancellation) that no reply announces.
    pub settle: Duration,
    /// Waiting for traffic to resume after [`Broker::disrupt`].
    pub recovery: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            boot: Duration::from_secs(6),
            settle: Duration::from_secs(4),
            recovery: Duration::from_secs(20),
        }
    }
}

/// A live broker, and the two halves of ulo that talk to it.
///
/// One implementation per transport crate, in that crate's `tests/`. The
/// implementor owns the container: holding `Self` keeps the broker alive, and
/// dropping it tears the broker down.
pub trait Broker: Sized + 'static {
    /// The server-side adapter under test.
    type Adapter: ulo::rpc::RpcAdapter;
    /// The client-side transport under test.
    type Transport: ulo::rpc::RpcClientTransport + 'static;

    /// Start a broker nothing else is using. Called once per case, so state
    /// never leaks between them.
    fn start() -> impl Future<Output = Self>;

    /// A server adapter pointed at this broker.
    fn adapter(&self) -> Self::Adapter;

    /// A client transport pointed at this broker, with its call timeout
    /// already set.
    fn transport(&self) -> Self::Transport;

    /// Sever the connection in whatever way this broker allows — killing
    /// client connections, pausing the container. The case that follows
    /// asserts traffic resumes, which is the claim; how the connection broke
    /// is the transport's business.
    fn disrupt(&self) -> impl Future<Output = ()>;

    /// Override for a broker that needs longer than [`Budget::default`].
    fn budget() -> Budget {
        Budget::default()
    }
}

// ---------------------------------------------------------------------------
// Handlers the cases dispatch to.
//
// Each case starts its own application, so these counters are read only by the
// case that just reset them.
// ---------------------------------------------------------------------------

static EMITS: AtomicUsize = AtomicUsize::new(0);
static PRODUCER_SAW_CANCEL: AtomicBool = AtomicBool::new(false);

#[controller]
pub struct ConformanceController {}

#[patterns]
impl ConformanceController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("math.add")]
    async fn add(&self, data: RpcData) -> Result<RpcData, RpcError> {
        let v = data.as_json().cloned().unwrap_or_default();
        let a = v["a"].as_i64().unwrap_or(0);
        let b = v["b"].as_i64().unwrap_or(0);
        Ok(RpcData::json(serde_json::json!({ "sum": a + b })))
    }

    #[message_pattern("echo")]
    async fn echo(&self, data: RpcData) -> Result<RpcData, RpcError> {
        Ok(data)
    }

    #[message_pattern("meta.echo")]
    async fn meta_echo(&self, _d: RpcData, c: &RpcContext) -> Result<RpcData, RpcError> {
        let trace = c.header("trace").unwrap_or("none").to_string();
        Ok(RpcData::json(serde_json::json!({ "trace": trace })))
    }

    #[event_pattern("event.fire")]
    async fn fire(&self, _d: RpcData) -> Result<(), RpcError> {
        EMITS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[message_pattern("count.stream")]
    async fn count(&self, _d: RpcData) -> RpcHandlerResult {
        Ok(Cardinality::Many(
            futures::stream::iter((1..=3).map(|n| Ok(RpcData::json(serde_json::json!(n))))).boxed(),
        ))
    }

    /// Emits until the execution is cancelled, then records that it noticed.
    /// A bounded channel of one means the producer cannot run ahead of the
    /// consumer, so the drop is observed promptly.
    #[message_pattern("probe.cancel")]
    async fn probe_cancel(&self, _d: RpcData, ctx: &RpcContext) -> RpcHandlerResult {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<RpcData, RpcError>>(1);
        let token = ctx.cancellation().clone();
        tokio::spawn(async move {
            let mut n = 0u32;
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        PRODUCER_SAW_CANCEL.store(true, Ordering::SeqCst);
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_millis(30)) => {
                        n += 1;
                        if tx.send(Ok(RpcData::json(serde_json::json!(n)))).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(Cardinality::Many(
            tokio_stream::wrappers::ReceiverStream::new(rx).boxed(),
        ))
    }
}

#[module(controllers: [ConformanceController])]
impl ConformanceModule {}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Boot an application on `broker`, run `body` against a client, tear down.
///
/// The application is `!Send` (its container is `Rc<RefCell<_>>`), so it lives
/// on a `LocalSet` for the duration of the case.
async fn with_server<B, F, Fut>(broker: &B, body: F)
where
    B: Broker,
    F: FnOnce(RpcClient) -> Fut,
    Fut: Future<Output = ()>,
{
    let adapter = broker.adapter();
    let client = RpcClient::new(broker.transport());

    tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let mut app = UloFactory::new()
                    .create_with(ConformanceModule)
                    .await
                    .expect("the conformance module builds");
                app.use_rpc_adapter(adapter)
                    .expect("the adapter is accepted while configuring");
                app.bind().await.expect("the transport binds");
                app.run().await;
            });

            body(client).await;
        })
        .await;
}

/// Poll `f` until it yields a value or `budget` runs out.
///
/// Every broker here subscribes asynchronously after `bind` returns, so the
/// first request can arrive before anything is listening. Retrying is the
/// difference between a suite that passes and one that passes on a fast
/// machine.
async fn within<T, F, Fut>(budget: Duration, mut f: F) -> Option<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<T>>,
{
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if let Some(v) = f().await {
            return Some(v);
        }
        if tokio::time::Instant::now() >= deadline {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// ---------------------------------------------------------------------------
// The cases
// ---------------------------------------------------------------------------

/// A request reaches the handler and its reply reaches the caller.
pub async fn send_round_trips<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    with_server(&broker, |client| async move {
        let sum = within(budget.boot, || async {
            client
                .send(
                    "math.add",
                    RpcData::json(serde_json::json!({"a": 2, "b": 3})),
                )
                .await
                .ok()
                .and_then(|r| r.as_json().and_then(|v| v["sum"].as_i64()))
        })
        .await;

        assert_eq!(sum, Some(5), "a request must come back with its reply");
    })
    .await;
}

/// An emit reaches its handler, and the caller does not wait for a reply the
/// handler never produces.
pub async fn emit_reaches_a_handler_with_no_reply<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    with_server(&broker, |client| async move {
        // Wait for the subscriptions before counting: an emit dropped because
        // nothing was listening yet is indistinguishable from one the handler
        // ignored.
        within(budget.boot, || async {
            client
                .send("echo", RpcData::json(serde_json::json!(1)))
                .await
                .ok()
        })
        .await
        .expect("the server must be reachable before the emit is meaningful");

        EMITS.store(0, Ordering::SeqCst);
        client
            .emit("event.fire", RpcData::json(serde_json::json!({})))
            .await
            .expect("emit must be accepted");

        let seen = within(budget.settle, || async {
            (EMITS.load(Ordering::SeqCst) == 1).then_some(())
        })
        .await;

        assert!(
            seen.is_some(),
            "emit must reach the fire-and-forget handler exactly once, saw {}",
            EMITS.load(Ordering::SeqCst)
        );
    })
    .await;
}

/// A header set on the request is readable from the handler's context.
pub async fn client_headers_reach_the_handler<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    with_server(&broker, |client| async move {
        let trace = within(budget.boot, || async {
            client
                .request("meta.echo")
                .header("trace", "abc123")
                .send(RpcData::json(serde_json::json!({})))
                .await
                .ok()
                .and_then(|r| {
                    r.as_json()
                        .and_then(|v| v["trace"].as_str().map(String::from))
                })
        })
        .await;

        assert_eq!(
            trace.as_deref(),
            Some("abc123"),
            "a header set on the request must reach the handler"
        );
    })
    .await;
}

/// A streaming reply arrives in order and terminates.
pub async fn a_stream_arrives_in_order_and_ends<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    with_server(&broker, |client| async move {
        let items = within(budget.boot, || async {
            let stream = client
                .stream("count.stream", RpcData::json(serde_json::json!(null)))
                .await
                .ok()?;
            let items: Vec<i64> = stream
                .filter_map(|item| async move {
                    item.ok().and_then(|d| d.as_json().and_then(|v| v.as_i64()))
                })
                .collect()
                .await;
            (!items.is_empty()).then_some(items)
        })
        .await;

        assert_eq!(
            items,
            Some(vec![1, 2, 3]),
            "a stream must deliver every item in order and then end"
        );
    })
    .await;
}

/// Dropping the reply stream cancels the execution producing it.
///
/// The producer is a detached task; without the cancel notice reaching it, it
/// runs until the process ends. That is the leak this case exists to catch.
pub async fn dropping_the_reply_stream_cancels_the_producer<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    with_server(&broker, |client| async move {
        PRODUCER_SAW_CANCEL.store(false, Ordering::SeqCst);

        // Warm up first. Cancellation travels its own carrier — a channel, a
        // subject, an in-band frame — which several transports establish
        // lazily on first use. Opening the probe before the transport is
        // demonstrably carrying calls tests the warm-up, not the cancel.
        within(budget.boot, || async {
            client
                .send("echo", RpcData::json(serde_json::json!(1)))
                .await
                .ok()
        })
        .await
        .expect("the transport must be carrying calls before cancellation means anything");

        let mut stream = client
            .stream("probe.cancel", RpcData::json(serde_json::json!(null)))
            .await
            .expect("the probe stream opens");
        assert!(
            stream.next().await.is_some(),
            "the producer must deliver a first item before being abandoned"
        );
        drop(stream);

        let cancelled = within(budget.settle, || async {
            PRODUCER_SAW_CANCEL.load(Ordering::SeqCst).then_some(())
        })
        .await;

        assert!(
            cancelled.is_some(),
            "abandoning the reply stream must cancel the execution behind it"
        );
    })
    .await;
}

/// Traffic resumes after the connection breaks.
///
/// Both sides have to recover: the server resubscribes to its patterns and the
/// client re-establishes whatever carries replies. A transport that recovers
/// only one of the two passes its first request and then goes quiet.
pub async fn traffic_recovers_after_a_disruption<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    let echoes = |client: RpcClient, budget: Duration| async move {
        within(budget, || async {
            client
                .send("echo", RpcData::json(serde_json::json!({"v": 1})))
                .await
                .ok()
                .and_then(|r| r.as_json().and_then(|v| v["v"].as_i64()))
                .filter(|v| *v == 1)
        })
        .await
        .is_some()
    };

    let adapter = broker.adapter();
    let client = RpcClient::new(broker.transport());

    tokio::task::LocalSet::new()
        .run_until(async {
            tokio::task::spawn_local(async move {
                let mut app = UloFactory::new()
                    .create_with(ConformanceModule)
                    .await
                    .expect("the conformance module builds");
                app.use_rpc_adapter(adapter)
                    .expect("the adapter is accepted while configuring");
                app.bind().await.expect("the transport binds");
                app.run().await;
            });

            assert!(
                echoes(client.clone(), budget.boot).await,
                "echo must round-trip before the disruption, or the case proves nothing"
            );

            broker.disrupt().await;

            assert!(
                echoes(client.clone(), budget.recovery).await,
                "echo must round-trip again after the connection is severed"
            );
        })
        .await;
}

/// Stamp one `#[tokio::test]` per case for a [`Broker`] implementation.
///
/// A case added to this crate reaches every transport through this macro,
/// which is the property seven hand-maintained copies did not have.
#[macro_export]
macro_rules! conformance_suite {
    ($broker:ty) => {
        $crate::conformance_suite!($broker, cases: [
            send_round_trips,
            emit_reaches_a_handler_with_no_reply,
            client_headers_reach_the_handler,
            a_stream_arrives_in_order_and_ends,
            dropping_the_reply_stream_cancels_the_producer,
            traffic_recovers_after_a_disruption,
        ]);
    };
    ($broker:ty, cases: [$($case:ident),* $(,)?]) => {
        $(
            #[tokio::test]
            async fn $case() {
                $crate::$case::<$broker>().await;
            }
        )*
    };
}
