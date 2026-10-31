//! The RPC conformance suite over the RabbitMQ link, each scenario against a RabbitMQ broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts.
//!
//! Beside the suite, three tests drop the link without `close` and count, through a relay, the
//! connections the broker still holds: each side's link, and a server's link holding a call nobody
//! answers. A fourth has the broker close every connection and shows the server's recovers, and
//! a fifth loses the broker between the server's two connections and shows `listen` fails at its
//! `connect_within`.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use testcontainers::core::{CmdWaitFor, ExecCommand, IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::{App, BoundAddr, Module, ModuleDef, ModuleIdentity, Runtime, Signal, StartupError};
use ulo_rpc::__private::ClientProbe;
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern, ReplyTo, Server};
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc_rabbitmq::{ConnectTimedOut, RabbitMq};
use ulo_runtime_tokio::Tokio;

const PORT: u16 = 5672;

struct RabbitMqBroker {
    server: SocketAddr,
    relay: Relay,
    _container: ContainerAsync<GenericImage>,
}

impl Broker for RabbitMqBroker {
    type Harness = ulo_rpc_conformance::OnTokio;
    type Link = RabbitMq;

    fn probe(link: &RabbitMq) -> Option<&ClientProbe> {
        Some(link.probe())
    }

    async fn start() -> Self {
        let (container, addrs) = unshadowed("the RabbitMQ container", || async {
            let container = GenericImage::new("rabbitmq", "4.1-alpine")
                .with_exposed_port(PORT.tcp())
                .with_wait_for(WaitFor::message_on_stdout("Server startup complete"))
                .start()
                .await
                .unwrap_or_else(|error| startup_failed!("the RabbitMQ container did not start: {}", report(&error)));
            let port = container.get_host_port_ipv4(PORT).await
                .unwrap_or_else(|error| startup_failed!("the AMQP port is not mapped: {}", report(&error)));
            (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, port))])
        })
        .await;
        let server = addrs[0];
        reachable(server, Duration::from_secs(10)).await;
        RabbitMqBroker { server, relay: Relay::start(server).await, _container: container }
    }

    fn link(&self) -> RabbitMq {
        RabbitMq::url(format!("amqp://guest:guest@{}/%2f", self.server))
    }

    fn client_link(&self, _server: &[BoundAddr]) -> RabbitMq {
        RabbitMq::url(format!("amqp://guest:guest@{}/%2f", self.relay.addr()))
    }

    async fn disrupt(&self) {
        self.relay.cut().await;
    }

    async fn client_connections(&self) -> Option<usize> {
        Some(self.relay.open().await)
    }
}

ulo_rpc_conformance::conformance_suite!(RabbitMqBroker; not_applicable {
    cancel_follows_its_request: "U17: the link carries a request and its `cancel` on separate lanes, and the broker may deliver the `cancel` first",
});

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
/// step, closes its connection: the tasks consuming its queues each hold a channel, which keeps the
/// connection open, and a spawned task runs on with no handle. The link reaches the broker through
/// a relay of its own, which counts the connections the broker still holds.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_without_close_closes_its_connection() {
    let broker = RabbitMqBroker::start().await;
    let relay = Relay::start(broker.server).await;
    let server = RabbitMq::url(format!("amqp://guest:guest@{}/%2f", relay.addr()));
    let inbound = server.listen(&[Pattern::from("link.dropped")]).await.expect("the server's link listens");
    assert!(relay.open().await > 0, "the listening server's link holds no connection through its relay");
    drop(inbound);
    drop(server);
    released(&relay, "the server's link dropped without `close`").await;
}

/// F428: a client's link dropped without `close`, with the outbound it answered, as a dropped
/// `RpcClient` whose spawned close never ran drops them, closes its connection: the reply router
/// holds the reply consumer and a channel, and ends only when the connection does.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_link_dropped_without_close_closes_its_connection() {
    let broker = RabbitMqBroker::start().await;
    let client = broker.client_link(&[]);
    let runtime: Arc<dyn Runtime> = Arc::new(ulo_runtime_tokio::Tokio::current());
    let outbound = client.connect(&runtime).await.expect("the client's link connects");
    assert!(broker.relay.open().await > 0, "the connected client's link holds no connection through its relay");
    drop(outbound);
    drop(client);
    released(&broker.relay, "the client's link dropped without `close`").await;
}

/// How long a request has to reach the server's link.
const PATIENCE: Duration = Duration::from_secs(10);

/// F429: a server's link dropped holding a call whose delivery nobody answers closes its
/// connection, undrained and drained. A held call's reply path holds the writer's queue and the
/// table the call is held in, and the table holds the path; the writer holds a channel, which
/// keeps the connection open. The side's drop clears the table, and the drain's watcher, which
/// waits on the table, holds the side weakly. The request comes from a client connected to the
/// broker directly, so the server's relay counts the server's connection alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_link_dropped_holding_a_call_closes_its_connection() {
    let broker = RabbitMqBroker::start().await;
    let client = broker.link();
    let runtime: Arc<dyn Runtime> = Arc::new(ulo_runtime_tokio::Tokio::current());
    let outbound = client.connect(&runtime).await.expect("the client's link connects");
    for (id, drained) in [(1, false), (2, true)] {
        let pattern = if drained { "link.held.drained" } else { "link.held" };
        let relay = Relay::start(broker.server).await;
        let server = RabbitMq::url(format!("amqp://guest:guest@{}/%2f", relay.addr()));
        let mut inbound = server.listen(&[Pattern::from(pattern)]).await.expect("the server's link listens");
        let request = Frame::Req { id, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
        (outbound.send)(Pattern::from(pattern), request, Some(ReplyTo { id })).await.expect("the request is published");
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

/// The peer ports of the connections the broker holds, read with `rabbitmqctl`.
async fn peer_ports(broker: &RabbitMqBroker) -> Vec<u16> {
    let listed = rabbitmqctl(broker, &["list_connections", "-q", "--no-table-headers", "peer_port"]).await;
    listed.split_whitespace().map(|port| port.parse().unwrap_or_else(|_| panic!("`rabbitmqctl` listed `{port}` as a peer port"))).collect()
}

/// `rabbitmqctl` in the broker's container. A CLI that hangs, which one run under load did, fails
/// the test at `PATIENCE` rather than holding it.
async fn rabbitmqctl(broker: &RabbitMqBroker, args: &[&str]) -> String {
    let command = ExecCommand::new(std::iter::once("rabbitmqctl").chain(args.iter().copied())).with_cmd_ready_condition(CmdWaitFor::exit_code(0));
    let ran = async {
        let mut ran = broker._container.exec(command).await.unwrap_or_else(|error| panic!("`rabbitmqctl {}` failed: {}", args.join(" "), report(&error)));
        ran.stdout_to_vec().await.unwrap_or_else(|error| panic!("`rabbitmqctl {}`'s output: {}", args.join(" "), report(&error)))
    };
    let stdout = tokio::time::timeout(PATIENCE, ran).await.unwrap_or_else(|_| panic!("`rabbitmqctl {}` did not finish within {PATIENCE:?}", args.join(" ")));
    String::from_utf8_lossy(&stdout).into_owned()
}

/// Q17-P3: the server's link checks the broker on a connection without lapin's auto-recovery,
/// closes it, and serves on one that recovers. Once `listen` returns the broker holds the server's
/// one connection, and after the broker closes every connection the same inbound stream takes a
/// request on a connection it had not held before and the delivery's reply path answers it.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_s_connection_recovers_after_the_broker_closes_it() {
    let broker = RabbitMqBroker::start().await;
    let server = broker.link();
    let pattern = "link.recovered";
    let mut inbound = server.listen(&[Pattern::from(pattern)]).await.expect("the server's link listens");
    let listening = std::time::Instant::now();
    let held = loop {
        let held = peer_ports(&broker).await;
        if held.len() == 1 {
            break held;
        }
        assert!(listening.elapsed() < RELEASE, "the broker holds {} connections for the listening server, not one: {held:?}", held.len());
        tokio::time::sleep(Duration::from_millis(100)).await;
    };

    rabbitmqctl(&broker, &["close_all_connections", "the recovery test"]).await;

    let client = broker.link();
    let runtime: Arc<dyn Runtime> = Arc::new(ulo_runtime_tokio::Tokio::current());
    let mut outbound = client.connect(&runtime).await.expect("the client's link connects");
    let request = Frame::Req { id: 1, pattern: pattern.to_owned(), headers: CallHeaders::new(), data: Data::new(b"null".as_slice()) };
    (outbound.send)(Pattern::from(pattern), request, Some(ReplyTo { id: 1 })).await.expect("the request is published");
    let delivery = tokio::time::timeout(PATIENCE, inbound.next())
        .await
        .unwrap_or_else(|_| panic!("the request did not reach the recovered server within {PATIENCE:?}"))
        .expect("the server's inbound stream ended at the broker's close");
    let Frame::Req { id, .. } = delivery.frame else { panic!("the server received something other than the request: {:?}", delivery.frame) };
    let reply = delivery.reply.as_ref().expect("a request carries its reply path");
    reply.send(Frame::Res { id, data: Data::new(b"7".as_slice()) }).await.expect("the recovered server's reply is published");
    let answer = tokio::time::timeout(PATIENCE, outbound.replies.next())
        .await
        .unwrap_or_else(|_| panic!("the recovered server's reply did not arrive within {PATIENCE:?}"))
        .expect("the client's reply lane ended");
    assert!(matches!(&answer, Frame::Res { id: 1, data } if data.as_bytes() == b"7"), "the client received something other than the reply: {answer:?}");

    let after = peer_ports(&broker).await;
    assert!(after.iter().all(|port| !held.contains(port)), "the server's connection from before the close is still held: {held:?} then {after:?}");
    delivery.ack.ack();
    client.close().await.expect("the client's link closes");
    server.close().await.expect("the server's link closes");
}

/// Forwards the first connection made to it to `upstream` and then stops listening, so every later
/// connect is refused, as it is once the broker is gone. `cut_for` closes connections the relay
/// accepts rather than refusing them, and it starts at a moment of the test's choosing, which
/// cannot fall between `listen`'s close of its first connection and its second connect.
fn relay_first_connection(upstream: SocketAddr) -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
    let addr = listener.local_addr().expect("the relay's address");
    std::thread::spawn(move || {
        let Ok((client, _)) = listener.accept() else { return };
        drop(listener);
        let Ok(broker) = TcpStream::connect(upstream) else { return };
        let pairs = [(client.try_clone(), broker.try_clone()), (broker.try_clone(), client.try_clone())];
        for (from, to) in pairs {
            let (Ok(mut from), Ok(mut to)) = (from, to) else { return };
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut from, &mut to);
                let _ = to.shutdown(Shutdown::Write);
            });
        }
    });
    addr
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// The bound the broker-lost test gives `listen`, short of lapin's backoff, whose sixteen attempts
/// take minutes.
const LOST_WITHIN: Duration = Duration::from_secs(3);

/// How long past [`LOST_WITHIN`] the failure may take to arrive.
const LOST_MARGIN: Duration = Duration::from_secs(5);

/// Q19-P2: a broker lost after the server's first connection, which checks it and is closed, and
/// before the serving one, fails `listen` as `Bind` at `connect_within`, carrying
/// `ConnectTimedOut`, rather than holding it while lapin retries the serving connection's connect
/// on its backoff. The relay carries the first connection to the broker and refuses the second.
#[tokio::test(flavor = "multi_thread")]
async fn a_broker_lost_between_the_two_connections_fails_listen_at_the_bound() {
    let broker = RabbitMqBroker::start().await;
    let relay = relay_first_connection(broker.server);
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let link = RabbitMq::url(format!("amqp://guest:guest@{relay}/%2f")).connect_within(LOST_WITHIN);
    let started = Instant::now();
    // A hang is the failure: the bound is a watchdog, not a timing the link keeps.
    let watchdog = LOST_WITHIN + LOST_MARGIN + PATIENCE;
    let listened = tokio::time::timeout(watchdog, app.bind(Server::new(link)).listen()).await;
    let elapsed = started.elapsed();
    let error = match listened {
        Err(_) => panic!("`listen` with the broker lost after its first connection did not return within {watchdog:?}"),
        Ok(Ok(app)) => {
            let _ = app.handle().close(Signal::new("lost")).await;
            panic!("a server listened with its broker lost after the first connection");
        }
        Ok(Err(error)) => error,
    };
    assert!(!ulo::is_permanent(&error), "a lost broker is marked permanent, though a later start may reach it: {error}");
    let StartupError::Bind { source, .. } = error else { panic!("expected a `Bind` failure, got: {error}") };
    let cause = source.into_inner();
    let timed_out = cause.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("the `Bind` does not carry `ConnectTimedOut`: {cause:?}"));
    assert_eq!((timed_out.within(), timed_out.address()), (LOST_WITHIN, relay.to_string().as_str()));
    assert!(
        elapsed >= LOST_WITHIN && elapsed < LOST_WITHIN + LOST_MARGIN,
        "`listen` failed after {elapsed:?}, not at its {LOST_WITHIN:?} bound"
    );
}
