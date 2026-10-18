//! The RPC conformance suite over the Kafka link, each scenario against a single-node KRaft broker
//! in a container of its own.
//!
//! A Kafka client connects wherever the broker's metadata advertises, not where it bootstrapped,
//! so the broker has one listener for the server and one for the client, each advertising a relay
//! bound before the container starts. `disrupt` cuts the client's relay alone and keeps it shut
//! while the recovery scenario's held call is answered: the link declares `durable_replies`, and
//! the reply waits on the reply topic for the client to reconnect.
//!
//! At most `PARALLEL` brokers run at once: one per scenario, started together, compete for the
//! host's memory, and a container that runs short can exit before its ready line.
//!
//! Beside the suite, one test drives the link through `Link` directly: the server's `close`
//! commits the group's offsets on a thread of its own, so a frozen broker holds no runtime worker.
//! Three more drop the link without `close` and count, through its relay, the connections the
//! broker still holds: each side's link, and a server's link holding a call nobody answers. A last
//! one closes and drops links on one thread beside a ticker, which a librdkafka handle dropped on
//! that thread stalls.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};
use ulo::{App, BoundAddr, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern, ReplyTo};
use ulo_rpc::__private::ClientProbe;
use ulo_rpc_conformance::relay::{Outage, Relay, reachable, unshadowed};
use ulo_rpc_conformance::{Broker, Budget, Harness, OnTokio, report, startup_failed};
use ulo_rpc_kafka::Kafka;

/// The listener the server's link reaches.
const SERVER_PORT: u16 = 9092;
/// The listener the client's link reaches.
const CLIENT_PORT: u16 = 9095;
/// How long `disrupt` keeps the client disconnected: longer than the recovery scenario's held call
/// takes to be answered once `disrupt` begins, three seconds.
const OUTAGE: Duration = Duration::from_secs(4);

struct KraftBroker {
    server: Relay,
    client: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for KraftBroker {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = Kafka;

    fn probe(link: &Kafka) -> Option<&ClientProbe> {
        Some(link.probe())
    }

    const PARALLEL: Option<NonZeroUsize> = NonZeroUsize::new(4);

    async fn start() -> Self {
        let server = Relay::bind().await;
        let client = Relay::bind().await;
        let advertised = format!(
            "SERVER://{},CLIENT://{},BROKER://localhost:9094",
            server.local_addr().unwrap_or_else(|error| startup_failed!("the server relay has no address: {}", report(&error))),
            client.local_addr().unwrap_or_else(|error| startup_failed!("the client relay has no address: {}", report(&error))),
        );
        let (container, addrs) = unshadowed("the Kafka container", || {
            let advertised = advertised.clone();
            async move {
                let container = GenericImage::new("apache/kafka-native", "3.8.0")
                    .with_exposed_port(SERVER_PORT.tcp())
                    .with_exposed_port(CLIENT_PORT.tcp())
                    .with_wait_for(WaitFor::message_on_stdout("Kafka Server started"))
                    .with_env_var("CLUSTER_ID", "ulo-rpc-conformance-kafka")
                    .with_env_var("KAFKA_NODE_ID", "1")
                    .with_env_var("KAFKA_PROCESS_ROLES", "broker,controller")
                    .with_env_var("KAFKA_CONTROLLER_QUORUM_VOTERS", "1@localhost:9093")
                    .with_env_var("KAFKA_CONTROLLER_LISTENER_NAMES", "CONTROLLER")
                    .with_env_var("KAFKA_LISTENERS", "SERVER://:9092,CLIENT://:9095,CONTROLLER://:9093,BROKER://:9094")
                    .with_env_var("KAFKA_ADVERTISED_LISTENERS", advertised)
                    .with_env_var(
                        "KAFKA_LISTENER_SECURITY_PROTOCOL_MAP",
                        "SERVER:PLAINTEXT,CLIENT:PLAINTEXT,CONTROLLER:PLAINTEXT,BROKER:PLAINTEXT",
                    )
                    .with_env_var("KAFKA_INTER_BROKER_LISTENER_NAME", "BROKER")
                    .with_env_var("KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR", "1")
                    .with_env_var("KAFKA_OFFSETS_TOPIC_NUM_PARTITIONS", "1")
                    .with_env_var("KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR", "1")
                    .with_env_var("KAFKA_TRANSACTION_STATE_LOG_MIN_ISR", "1")
                    // A group's first join otherwise waits three seconds for more members.
                    .with_env_var("KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS", "0")
                    .start()
                    .await
                    .unwrap_or_else(|error| startup_failed!("the Kafka container did not start: {}", report(&error)));
                let mut addrs = Vec::new();
                for port in [SERVER_PORT, CLIENT_PORT] {
                    let port = container.get_host_port_ipv4(port).await
                        .unwrap_or_else(|error| startup_failed!("the Kafka listener is not mapped: {}", report(&error)));
                    addrs.push(SocketAddr::from((Ipv4Addr::LOCALHOST, port)));
                }
                (container, addrs)
            }
        })
        .await;
        for addr in &addrs {
            reachable(*addr, Duration::from_secs(10)).await;
        }
        let server = Relay::listen(server, addrs[0]);
        let client = Relay::listen(client, addrs[1]);
        KraftBroker { server, client, _container: container }
    }

    fn link(&self) -> Kafka {
        Kafka::brokers(self.server.addr().to_string())
    }

    fn client_link(&self, _server: &[BoundAddr]) -> Kafka {
        Kafka::brokers(self.client.addr().to_string())
    }

    /// Holds the client out for `OUTAGE`, past the moment the recovery scenario's held call is
    /// answered, so its reply is published while the client is disconnected.
    async fn disrupt(&self) {
        let severed = self.client.cut_for(OUTAGE).await;
        assert!(severed > 0, "`disrupt` found no client connection to sever");
    }

    fn outage(&self) -> Option<Outage> {
        self.client.last_outage()
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.client.open().await)
    }

    /// A consumer group takes its partitions a few seconds after `bind`, and a record waits for
    /// the next fetch.
    fn budget(&self) -> Budget {
        Budget::new(Duration::from_secs(30), Duration::from_secs(3), Duration::from_secs(30))
    }
}

ulo_rpc_conformance::conformance_suite!(KraftBroker; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});

/// An app with nothing in it, whose handle the server's link is prepared with.
struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// How long the runtime polling `close` is watched with the broker frozen.
const FROZEN: Duration = Duration::from_secs(3);

/// The ticks of a 50 ms timer that one second of a free runtime thread allows, at the least.
const FREE_TICKS: u64 = 20;

/// F370: the server's `close` commits the group's stored offsets synchronously, which blocks the
/// calling thread until the broker answers, or until librdkafka's request timeout once it cannot.
/// With an offset stored and the broker's container paused, `close` is polled on a current-thread
/// runtime beside a task ticking every 50 ms: the commit runs on a thread of its own, so the task
/// keeps ticking while `close` waits. The broker resumes a second after the watch ends, so a commit
/// left on the closing thread, which librdkafka holds without bound while the broker is frozen,
/// ends then and the test reports it.
#[tokio::test(flavor = "multi_thread")]
async fn close_commits_off_the_runtime_thread_polling_it() {
    const PATTERN: &str = "f370.call";
    let broker = KraftBroker::start().await;
    let app = App::builder(Empty)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut server = broker.link();
    server.prepare(&app.handle()).await.expect("the server's link prepares");
    let mut inbound = server.listen(&[Pattern::from(PATTERN)]).await.expect("the server's link listens");
    let client = broker.client_link(&[]);
    let handle = app.handle();
    let outbound = client.connect(handle.runtime().expect("the app has a runtime")).await.expect("the client's link connects");
    let request = Frame::Req { id: 1, pattern: PATTERN.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    (outbound.send)(Pattern::from(PATTERN), request, Some(ReplyTo { id: 1 })).await.expect("the request is produced");
    let delivery = tokio::time::timeout(Duration::from_secs(60), inbound.next())
        .await
        .expect("the request did not reach the server within a minute")
        .expect("the server's inbound stream ended");
    // Stores the record's offset, which the close then commits.
    delivery.ack.ack();
    client.close().await.expect("the client's link closes");

    broker._container.pause().await.expect("the broker's container pauses");
    let ticks = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&ticks);
    let closing = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_time().build().expect("a runtime for the close");
        runtime.block_on(async move {
            let ticker = tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    counted.fetch_add(1, Ordering::SeqCst);
                }
            });
            let started = Instant::now();
            let _ = tokio::time::timeout(FROZEN, server.close()).await;
            ticker.abort();
            started.elapsed()
        })
    });
    // Resumed whatever `close` did, so a commit holding the closing thread ends once the broker
    // answers it: frozen, librdkafka waits on a synchronous commit without bound.
    tokio::time::sleep(FROZEN + Duration::from_secs(1)).await;
    broker._container.unpause().await.expect("the broker's container resumes");
    let joined = tokio::time::timeout(Duration::from_secs(60), tokio::task::spawn_blocking(move || closing.join()))
        .await
        .expect("the closing thread did not end within a minute of the broker resuming");
    let took = joined.expect("the join completes").expect("the closing thread panicked");
    let ticked = ticks.load(Ordering::SeqCst);
    assert!(
        ticked >= FREE_TICKS,
        "the runtime polling `close` ran its other task {ticked} times in {took:?} with the broker frozen: the commit held its thread"
    );
    let _ = app.close(Signal::new("conformance")).await;
}

/// How long a dropped link's connections have to close at the broker: each consumer leaves its
/// group from a thread of its own before its connections close.
const RELEASE: Duration = Duration::from_secs(30);

/// Waits until `relay` carries no connection, the broker having closed each of its ends in turn,
/// failing the test with `what` and the count left after [`RELEASE`].
async fn released(relay: &Relay, what: &str) {
    let dropped = Instant::now();
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
/// step, closes its connections: the tasks reading its two consumers hold its side, and a spawned
/// task runs on with no handle, its consumers still members of their groups. The server's link
/// reaches the broker through the relay its listener advertises, which counts the connections the
/// broker still holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_without_close_closes_its_connections() {
    let broker = KraftBroker::start().await;
    let app = App::builder(Empty)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let mut server = broker.link().group("dropped");
    server.prepare(&app.handle()).await.expect("the server's link prepares");
    let inbound = server.listen(&[Pattern::from("link.dropped")]).await.expect("the server's link listens");
    assert!(broker.server.open().await > 0, "the listening server's link holds no connection through its relay");
    drop(inbound);
    drop(server);
    released(&broker.server, "the server's link dropped without `close`").await;
    let _ = app.close(Signal::new("conformance")).await;
}

/// F428: a client's link dropped without `close`, with the outbound it answered, as a dropped
/// `RpcClient` whose spawned close never ran drops them, closes its connections: the reply router
/// holds the reply consumer and ends only at `close` or on a reply it cannot hand over.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_link_dropped_without_close_closes_its_connections() {
    let broker = KraftBroker::start().await;
    let client = broker.client_link(&[]);
    let runtime: Arc<dyn ulo::Runtime> = Arc::new(ulo_runtime_tokio::Tokio::current());
    let outbound = client.connect(&runtime).await.expect("the client's link connects");
    assert!(broker.client.open().await > 0, "the connected client's link holds no connection through its relay");
    drop(outbound);
    drop(client);
    released(&broker.client, "the client's link dropped without `close`").await;
}

/// F429: a server's link dropped holding a call whose delivery nobody answers closes its
/// connections, undrained and drained. A held call's reply path holds the writer's queue and the
/// table the call is held in, and the table holds the path; the writer holds the producer. The
/// side's drop clears the table, and the drain's watcher, which waits on the table, holds the side
/// weakly. Each server answers one call before it holds the next: a producer that has produced
/// nothing holds no connection once librdkafka has read the broker's metadata, so only an answer
/// puts the producer's connection on the relay. The client reaches the broker through the client's
/// relay, so the server's relay counts the server's connections alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_holding_a_call_closes_its_connections() {
    let broker = KraftBroker::start().await;
    let app = App::builder(Empty)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    let client = broker.client_link(&[]);
    let handle = app.handle();
    let outbound = client.connect(handle.runtime().expect("the app has a runtime")).await.expect("the client's link connects");
    let (send, mut replies) = (outbound.send, outbound.replies);
    for (first, drained) in [(1, false), (3, true)] {
        let (pattern, group) = if drained { ("link.held.drained", "held.drained") } else { ("link.held", "held") };
        let mut server = broker.link().group(group);
        server.prepare(&app.handle()).await.expect("the server's link prepares");
        let mut inbound = server.listen(&[Pattern::from(pattern)]).await.expect("the server's link listens");
        let mut deliveries = Vec::new();
        for id in [first, first + 1] {
            let request = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
            send(Pattern::from(pattern), request, Some(ReplyTo { id })).await.expect("the request is produced");
            let delivery = tokio::time::timeout(Duration::from_secs(60), inbound.next())
                .await
                .expect("the request did not reach the server within a minute")
                .expect("the server's inbound stream ended");
            let Frame::Req { id: local, .. } = delivery.frame else {
                panic!("the server received something other than the request: {:?}", delivery.frame);
            };
            if id == first {
                let reply = delivery.reply.clone().expect("a request carries its reply path");
                reply.send(Frame::Res { id: local, data: Data::new(b"2".as_slice()) }).await.expect("the reply is produced");
                let answer = tokio::time::timeout(Duration::from_secs(30), replies.next()).await.ok().flatten();
                assert!(matches!(answer, Some(Frame::Res { id: answered, .. }) if answered == first), "the server's answer did not reach the client: {answer:?}");
            }
            deliveries.push(delivery);
        }
        if drained {
            server.drain().await;
        }
        drop(deliveries);
        drop(inbound);
        drop(server);
        let what = if drained { "the drained server's link dropped holding a call" } else { "the server's link dropped holding a call" };
        released(&broker.server, what).await;
    }
    client.close().await.expect("the client's link closes");
    let _ = app.close(Signal::new("conformance")).await;
}

/// How long [`closing_and_dropping_links_does_not_stall_the_thread`] lets its thread go without
/// polling the ticker: half the suite's [`Budget::stall`]. A server's producer dropped on the
/// thread held it 42.5 and 46.7 ms, under the suite's 50, and the thread's longest gap was 5.1 to
/// 13.6 ms with every producer dropped elsewhere.
const DROP_STALL: Duration = Duration::from_millis(25);

/// F430: a link's producer is dropped on a thread of its own, at the server's `close`, when a
/// server's link drops without it, and when a client's link drops without it. A producer's drop
/// joins its polling thread and flushes, which on the thread polling every task stalls each of
/// them. On one thread beside a ticker sleeping a tenth of [`DROP_STALL`] between ticks, a server
/// answers a call, so its producer holds a connection, and is closed; a second answers one and is
/// dropped; then the client that called both produces an event and is dropped. Each phase waits
/// until its relay carries no connection, so the producer's drop falls inside the ticker's window,
/// and the longest gap between two ticks in each phase stays under the bound. The conformance
/// probe's hooks are detached before the client's drop: they hold the client writer's queue, which
/// keeps the client's side, and with it the producer, alive in a probe build, where a production
/// build drops the side once its writer ends.
#[test]
fn closing_and_dropping_links_does_not_stall_the_thread() {
    OnTokio::block_on_one_thread(async {
        let broker = KraftBroker::start().await;
        let bound = DROP_STALL;
        let app = App::builder(Empty)
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .expect("the app wires")
            .connect()
            .await
            .expect("the app connects");
        let doing: Arc<Mutex<&'static str>> = Arc::new(Mutex::new("starting the ticker"));
        let stop = Arc::new(AtomicBool::new(false));
        // The longest gap in each phase, by the phase the gap ended in.
        let ticker = tokio::spawn({
            let (doing, stop) = (Arc::clone(&doing), Arc::clone(&stop));
            async move {
                let (mut longest, mut count, mut threads) = (Vec::<(&'static str, Duration)>::new(), 0usize, Vec::new());
                let mut last = Instant::now();
                while !stop.load(Ordering::Acquire) {
                    tokio::time::sleep(bound / 10).await;
                    let now = Instant::now();
                    let during = *doing.lock().unwrap_or_else(PoisonError::into_inner);
                    match longest.iter_mut().find(|(phase, _)| *phase == during) {
                        Some((_, gap)) => *gap = (*gap).max(now - last),
                        None => longest.push((during, now - last)),
                    }
                    last = now;
                    count += 1;
                    if !threads.contains(&std::thread::current().id()) {
                        threads.push(std::thread::current().id());
                    }
                }
                (longest, count, threads)
            }
        });
        let doing = |what: &'static str| *doing.lock().unwrap_or_else(PoisonError::into_inner) = what;

        doing("connecting the client");
        let client = broker.client_link(&[]);
        let handle = app.handle();
        let outbound = client.connect(handle.runtime().expect("the app has a runtime")).await.expect("the client's link connects");
        let (send, mut replies) = (outbound.send, outbound.replies);
        for (id, closed) in [(1, true), (2, false)] {
            let (pattern, group) = if closed { ("link.stall.closed", "stall.closed") } else { ("link.stall.dropped", "stall.dropped") };
            doing("binding a server");
            let mut server = broker.link().group(group);
            server.prepare(&app.handle()).await.expect("the server's link prepares");
            let mut inbound = server.listen(&[Pattern::from(pattern)]).await.expect("the server's link listens");
            doing("answering a call");
            let request = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
            send(Pattern::from(pattern), request, Some(ReplyTo { id })).await.expect("the request is produced");
            let delivery = tokio::time::timeout(Duration::from_secs(60), inbound.next())
                .await
                .expect("the request did not reach the server within a minute")
                .expect("the server's inbound stream ended");
            let Frame::Req { id: local, .. } = delivery.frame else {
                panic!("the server received something other than the request: {:?}", delivery.frame);
            };
            let reply = delivery.reply.clone().expect("a request carries its reply path");
            reply.send(Frame::Res { id: local, data: Data::new(b"2".as_slice()) }).await.expect("the reply is produced");
            let answer = tokio::time::timeout(Duration::from_secs(30), replies.next()).await.ok().flatten();
            assert!(matches!(answer, Some(Frame::Res { id: answered, .. }) if answered == id), "the server's answer did not reach the client: {answer:?}");
            // The delivery's acknowledgment holds the group's consumer.
            drop(delivery);
            drop(reply);
            drop(inbound);
            if closed {
                doing("closing a server");
                server.close().await.expect("the server's link closes");
                drop(server);
                released(&broker.server, "the closed server's link").await;
            } else {
                doing("dropping a server");
                drop(server);
                released(&broker.server, "the server's link dropped without `close`").await;
            }
        }
        // Its delivery report reaches the client's producer right before the drop. The producer's
        // polling thread polls a tenth of a second at a time, returns from a poll once it has
        // served an event and polls again, so a drop joining it now waits for most of a fresh
        // poll, where an idle producer's drop could come anywhere in one.
        let event = Frame::Evt { pattern: "link.stall.dropped".to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
        send(Pattern::from("link.stall.dropped"), event, None).await.expect("the event is produced");
        doing("dropping the client");
        client.probe().attach(|_| {}, || 0);
        drop(send);
        drop(replies);
        drop(client);
        released(&broker.client, "the client's link dropped without `close`").await;
        doing("stopping the ticker");
        stop.store(true, Ordering::Release);
        let (longest, count, threads) = ticker.await.expect("the ticker does not panic");
        let _ = app.close(Signal::new("conformance")).await;

        assert_eq!(threads, vec![std::thread::current().id()], "the ticker was polled on a thread other than the test's");
        assert!(count >= 1, "the ticker never ticked");
        let gaps: Vec<String> = longest.iter().map(|(phase, gap)| format!("{phase} {:.1} ms", gap.as_secs_f64() * 1e3)).collect();
        assert!(
            longest.iter().all(|(_, gap)| *gap < bound),
            "the test's thread went past the {:.1} ms bound without polling the ticker; the longest gap in each phase: {}",
            bound.as_secs_f64() * 1e3,
            gaps.join(", ")
        );
    });
}
