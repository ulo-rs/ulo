//! The RPC conformance suite over the Redis link, each scenario against a Redis server in a
//! container of its own, since Pub/Sub channels span every database of a server. The client
//! reaches it through a relay that `disrupt` cuts.
//!
//! Beside the suite, a test drives the link through `Link` directly, for an ordering no scenario
//! reaches on its own: a request read after the drain ended the inbound stream. Three more drop
//! the link without `close` and count, through a relay, the connections Redis still holds: each
//! side's link, and a server's link holding a call nobody answers. Two more run two and three
//! server apps on one Redis and count what one request and one event do across them.

#![cfg(feature = "integration")]

use std::error::Error;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::StreamExt;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::{App, AppHandle, BoundAddr, Dep, Module, ModuleDef, ModuleIdentity, Runtime, Signal, controller, handlers};
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern, Payload, RpcClient, Server};
use ulo_rpc_redis::Redis;
use ulo_transport::{Classify, ErrorKind};

/// The runtime a client hands its link: the test's tokio runtime, which a tokio link ignores.
fn client_runtime() -> Arc<dyn Runtime> {
    Arc::new(ulo_runtime_tokio::Tokio::current())
}

const PORT: u16 = 6379;

struct RedisServer {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for RedisServer {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = Redis;

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the Redis container", || async {
            let container = GenericImage::new("redis", "7-alpine")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the Redis container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the Redis port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        RedisServer { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> Redis {
        Redis::url(format!("redis://{}", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Redis {
        Redis::url(format!("redis://{}", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(RedisServer; not_applicable {
    a_late_opened_releases_the_held_cancel: "U17: a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
    a_held_cancel_is_dropped_when_its_hold_runs_out: "U17: a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
    a_held_cancel_lasts_until_its_call_s_deadline: "U17: a request and its control frames share one ordered lane, so the link holds no `cancel` for an `opened`",
});

/// How long the test waits for a frame either side of the link should see.
const PATIENCE: Duration = Duration::from_secs(5);

/// A request Redis delivered before it processed the drain's UNSUBSCRIBE reaches the server, or
/// is refused `unavailable` by the link: the drain's watcher ends the inbound stream once no call
/// is held, and the server lane can read the request only after that. A link that held the
/// request and handed it to an ended stream would leave the caller to its own `Timeout`.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_read_after_the_drain_ended_the_inbound_stream_is_refused() {
    const DRAINED: &str = "link.drained";
    let broker = RedisServer::start().await;
    let server = broker.link();
    let mut inbound = server.listen(&[Pattern::from(DRAINED).into()]).await.expect("the server's link listens");
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;

    let request = Frame::Req { id: 1, pattern: DRAINED.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    (outbound.send)(Pattern::from(DRAINED), request, None).await.expect("the request is published");
    server.drain().await;

    let delivered = tokio::time::timeout(PATIENCE, inbound.next())
        .await
        .unwrap_or_else(|_| panic!("the inbound stream neither handed the request over nor ended within {PATIENCE:?}"));
    match delivered {
        Some(delivery) => {
            let Frame::Req { id, .. } = delivery.frame else {
                panic!("the server received something other than the request: {:?}", delivery.frame);
            };
            let reply = delivery.reply.expect("a request carries its reply path");
            reply.send(Frame::Res { id, data: Data::new(b"2".as_slice()) }).await.expect("the reply is published");
            let answer = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
            assert!(matches!(answer, Some(Frame::Res { id: 1, .. })), "the server's reply did not reach the caller, got: {answer:?}");
        }
        None => {
            let answer = tokio::time::timeout(PATIENCE, replies.next()).await.ok().flatten();
            assert!(
                matches!(&answer, Some(Frame::Err { id: 1, error }) if error.kind == ErrorKind::Unavailable),
                "a request read after the inbound stream ended was not refused `unavailable`, got: {answer:?}",
            );
        }
    }
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// How long a dropped link's connections have to close at the broker.
const RELEASE: Duration = Duration::from_secs(10);

/// Waits until `relay` carries no connection, the broker having closed each of its ends in turn,
/// failing the test with `what` and the count left after [`RELEASE`].
async fn released(relay: &Relay, what: &str) {
    let dropped = std::time::Instant::now();
    loop {
        let open = relay.open().await;
        if open == 0 {
            return;
        }
        assert!(dropped.elapsed() < RELEASE, "{what}: {open} connections still open at the broker {RELEASE:?} after the link's drop");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// F428: a server's link dropped without `close`, as it is when its app drops before the close
/// step, closes its connections: the Pub/Sub reader `close` aborts holds its side, which holds the
/// Pub/Sub sink and the writer's queue, and a dropped `AbortHandle` leaves it running. The link
/// reaches Redis through a relay of its own, which counts the connections Redis still holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_without_close_closes_its_connections() {
    let broker = RedisServer::start().await;
    let relay = Relay::start(broker.server).await;
    let server = Redis::url(format!("redis://{}", relay.addr()));
    let inbound = server.listen(&[Pattern::from("link.dropped").into()]).await.expect("the server's link listens");
    assert!(relay.open().await > 0, "the listening server's link holds no connection through its relay");
    drop(inbound);
    drop(server);
    released(&relay, "the server's link dropped without `close`").await;
}

/// F428: a client's link dropped without `close`, with the outbound it answered, as a dropped
/// `RpcClient` whose spawned close never ran drops them, closes its connections: the reply reader
/// holds the reply subscription and ends only at `close` or on a reply it cannot hand over.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_link_dropped_without_close_closes_its_connections() {
    let broker = RedisServer::start().await;
    let client = broker.client_link(&[]);
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    assert!(broker.relay.open().await > 0, "the connected client's link holds no connection through its relay");
    drop(outbound);
    drop(client);
    released(&broker.relay, "the client's link dropped without `close`").await;
}

/// F429: a server's link dropped holding a call whose delivery nobody answers closes its
/// connections, undrained and drained. A held call's reply path holds the writer's queue and the
/// table the call is held in, and the table holds the path; the writer holds the publisher
/// connection. The side's drop clears the table, and the drain's watcher, which waits on the
/// table, holds the side weakly. The request comes from a client connected to Redis directly, so
/// the server's relay counts the server's connections alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_holding_a_call_closes_its_connections() {
    let broker = RedisServer::start().await;
    let client = broker.link();
    let outbound = client.connect(&client_runtime()).await.expect("the client's link connects");
    for (id, drained) in [(1, false), (2, true)] {
        let pattern = if drained { "link.held.drained" } else { "link.held" };
        let relay = Relay::start(broker.server).await;
        let server = Redis::url(format!("redis://{}", relay.addr()));
        let mut inbound = server.listen(&[Pattern::from(pattern).into()]).await.expect("the server's link listens");
        let request = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
        (outbound.send)(Pattern::from(pattern), request, None).await.expect("the request is published");
        let delivery = tokio::time::timeout(PATIENCE, inbound.next())
            .await
            .unwrap_or_else(|_| panic!("the request did not reach the server within {PATIENCE:?}"))
            .expect("the server's inbound stream ended");
        assert!(matches!(delivery.frame, Frame::Req { .. }), "the server received something other than the request: {:?}", delivery.frame);
        if drained {
            server.drain().await;
        }
        drop(delivery);
        drop(inbound);
        drop(server);
        let what = if drained { "the drained server's link dropped holding a call" } else { "the server's link dropped holding a call" };
        released(&relay, what).await;
    }
    client.close().await.expect("the client's link closes");
}

/// What the fan-out apps' handlers count, one value shared by every app in a test.
#[derive(Default)]
struct Counts {
    requests: AtomicUsize,
    events: AtomicUsize,
}

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

#[controller]
struct Counted {
    counts: Dep<Arc<Counts>>,
}

#[handlers]
impl Counted {
    #[ulo_rpc::message("fan.request")]
    async fn request(&self, n: Payload<u32>) -> Result<u32, Never> {
        self.counts.requests.fetch_add(1, Ordering::SeqCst);
        Ok(n.0 + 1)
    }

    #[ulo_rpc::event("fan.event")]
    async fn event(&self, _n: Payload<u32>) -> Result<(), Never> {
        self.counts.events.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct FanOutRoot(Arc<Counts>);

impl Module for FanOutRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(Arc::clone(&self.0));
        m.controller::<Counted>();
    }
}

/// One server app on `url`, counting into `counts`, served on a task of its own.
async fn counted_server(url: &str, counts: &Arc<Counts>) -> (AppHandle, tokio::task::JoinHandle<()>) {
    let app = App::builder(FanOutRoot(Arc::clone(counts)))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(Server::new(Redis::url(url)))
        .listen()
        .await
        .map_err(Box::new)
        .expect("the server binds");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    (handle, serving)
}

/// How long the fan-out tests wait, once the count they expect has arrived, for one more.
const SETTLE: Duration = Duration::from_millis(500);

/// Waits until `count` reads `expected`, failing with `what` after [`PATIENCE`], then [`SETTLE`]
/// more, and answers what it reads then.
async fn settled(count: impl Fn() -> usize, expected: usize, what: &str) -> usize {
    let waited = std::time::Instant::now();
    while count() < expected {
        assert!(waited.elapsed() < PATIENCE, "{what}: {} of {expected} within {PATIENCE:?}", count());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(SETTLE).await;
    count()
}

/// Redis Pub/Sub hands a message to every connection subscribed to its channel, and each server
/// subscribes to its patterns' channels, so one request runs its handler on every server and each
/// publishes a reply; `RpcClient` answers the first and drops the rest. The link is declared
/// without `single_delivery` for it. An event reaches every server as well, the link's
/// `DeliveryMode::FanOut`.
async fn fan_out(servers: usize) {
    let capabilities = Redis::url("redis://127.0.0.1:6379").capabilities();
    assert!(!capabilities.single_delivery() && capabilities.why_not_single_delivery().is_some(), "{capabilities:?}");
    let broker = RedisServer::start().await;
    let url = format!("redis://{}", broker.server);
    let counts = Arc::new(Counts::default());
    let mut running = Vec::new();
    for _ in 0..servers {
        running.push(counted_server(&url, &counts).await);
    }

    let client = RpcClient::new(Redis::url(url.as_str()), client_runtime()).expect("the link is usable");
    let answer = tokio::time::timeout(PATIENCE, client.request::<_, u32>("fan.request", &1u32)).await.expect("the request answered in time");
    assert_eq!(answer.expect("the request is answered"), 2);
    let executions = settled(|| counts.requests.load(Ordering::SeqCst), servers, "executions of one request").await;
    assert_eq!(executions, servers, "one request's executions across {servers} servers");

    // The reply lane as the link hands it over, before `RpcClient` drops the second reply.
    let link = Redis::url(url.as_str());
    let outbound = link.connect(&client_runtime()).await.expect("the client's link connects");
    let mut replies = outbound.replies;
    let request = Frame::Req { id: 7, pattern: "fan.request".to_owned(), headers: CallHeaders::new(), data: Data::new(b"1".as_slice()) };
    (outbound.send)(Pattern::from("fan.request"), request, None).await.expect("the request is published");
    let mut answered = 0;
    while let Ok(Some(frame)) = tokio::time::timeout(PATIENCE.min(SETTLE * 2), replies.next()).await {
        assert!(matches!(frame, Frame::Res { id: 7, .. }), "a reply other than the request's: {frame:?}");
        answered += 1;
    }
    assert_eq!(answered, servers, "replies to one request across {servers} servers");
    assert_eq!(counts.requests.load(Ordering::SeqCst), 2 * servers, "executions of two requests across {servers} servers");

    client.emit("fan.event", &1u32).await.expect("the event is published");
    let events = settled(|| counts.events.load(Ordering::SeqCst), servers, "deliveries of one event").await;
    assert_eq!(events, servers, "one event's deliveries across {servers} servers");

    link.close().await.expect("the client's link closes");
    for (handle, serving) in running {
        let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("the server closed in time");
        let _ = tokio::time::timeout(PATIENCE, serving).await.expect("the server's serve ended in time");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn one_request_runs_on_each_of_two_servers() {
    fan_out(2).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_request_runs_on_each_of_three_servers() {
    fan_out(3).await;
}

mod tasks;

/// Q22-P10: a panic in each of the server's tasks, its Pub/Sub reader and its writer, ends the
/// server as a transport failure naming the link and the task, the panic redacted.
#[tokio::test]
async fn a_panicking_server_task_is_a_panicked_failure_the_core_recognises() {
    let broker = RedisServer::start().await;
    for task in ["Pub/Sub reader", "writer"] {
        let plant = ulo_rpc::link::PlantedPanic::new(task, tasks::PANIC_TEXT);
        tasks::server_task_panics(broker.link().panic_in(plant.clone()), plant, &format!("the Redis link's {task} panicked: ")).await;
    }
}

/// Q22-P10: a panic in each of the client's tasks, its Pub/Sub reader and its writer, fails the
/// call waiting on it holding the panic.
#[tokio::test]
async fn a_panicking_client_task_fails_the_waiting_call_holding_the_redacted_panic() {
    let broker = RedisServer::start().await;
    for task in ["Pub/Sub reader", "writer"] {
        let plant = ulo_rpc::link::PlantedPanic::new(task, tasks::PANIC_TEXT);
        tasks::client_task_panics(broker.link(), broker.link().panic_in(plant.clone()), plant, &format!("the Redis link's {task} panicked: ")).await;
    }
}

/// F585: the link's readers and writers end with their owner, closed or dropped.
#[tokio::test]
async fn start_stop_cycles_leave_no_task_running() {
    let broker = RedisServer::start().await;
    tasks::cycles_leave_no_task_running(|handle| broker.link().with_handle(handle), Redis::live_tasks, tasks::Counted::LinkAndRuntime).await;
}

/// F585: a server writer holding answers its broker does not take ends when its link drops
/// without `close`: the drop aborts it. Without the abort the writer outlives the link by each
/// queued answer's publish timing out in turn, half a second apiece at redis-rs's default.
mod writer_abort {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use tokio::sync::watch;
    use ulo::{App, Dep, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
    use ulo_rpc::RpcClient;
    use ulo_runtime_tokio::Tokio;

    use super::*;

    const CALLS: usize = 30;

    #[derive(Clone)]
    pub struct Gate {
        entered: Arc<AtomicUsize>,
        open: watch::Receiver<bool>,
    }

    #[controller]
    pub struct Gated {
        gate: Dep<Gate>,
    }

    #[handlers]
    impl Gated {
        /// Answers once the test opens its gate.
        #[ulo_rpc::message("writer.gated")]
        async fn gated(&self) -> Result<u32, super::tasks::Never> {
            self.gate.entered.fetch_add(1, Ordering::SeqCst);
            let mut open = self.gate.open.clone();
            let _ = open.wait_for(|open| *open).await;
            Ok(7)
        }
    }

    pub struct Root(Gate);

    impl Module for Root {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, m: &mut ModuleDef<'_>) {
            m.value(self.0.clone());
            m.controller::<Gated>();
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_server_writer_holding_answers_a_silent_broker_ends_when_its_link_drops() {
        let broker = RedisServer::start().await;
        let tasks = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("the link's runtime");
        let link = broker.link().with_handle(tasks.handle().clone());
        let live = link.live_tasks();
        let (opening, open) = watch::channel(false);
        let gate = Gate { entered: Arc::default(), open };
        let app = App::builder(Root(gate.clone()))
            .runtime(Tokio::current())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects")
            .bind(ulo_rpc::Server::new(link))
            .listen()
            .await
            .map_err(Box::new)
            .expect("the server binds");
        let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
        let rpc = RpcClient::new(broker.link(), Arc::new(Tokio::current())).expect("the link is usable");
        let calling: Vec<_> = (0..CALLS)
            .map(|_| {
                let caller = rpc.clone();
                tokio::spawn(async move { caller.request::<_, u32>("writer.gated", &()).timeout(Duration::from_secs(60)).await })
            })
            .collect();
        tokio::time::timeout(Duration::from_secs(30), async {
            while gate.entered.load(Ordering::SeqCst) < CALLS {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("every call reached its handler");
        broker._container.pause().await.expect("the broker pauses");
        opening.send_replace(true);
        tokio::time::sleep(Duration::from_millis(300)).await;
        let before = live.load(Ordering::Acquire);
        serving.abort();
        let _ = serving.await;
        let waited = tokio::time::timeout(Duration::from_secs(5), async {
            while live.load(Ordering::Acquire) != 0 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        let after = live.load(Ordering::Acquire);
        broker._container.unpause().await.expect("the broker resumes");
        calling.iter().for_each(tokio::task::JoinHandle::abort);
        drop(rpc);
        tasks.shutdown_background();
        assert!(before > 0, "no link task was alive before the drop");
        assert!(waited.is_ok(), "{after} of the link's tasks alive 5 s after its app dropped, {before} before");
    }
}
