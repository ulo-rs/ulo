//! The RPC conformance suite over the RabbitMQ link, each scenario against a RabbitMQ broker in a
//! container of its own, the client reaching it through a relay that `disrupt` cuts.
//!
//! Beside the suite, two tests drop each side of the link without `close` and count, through a
//! relay, the connections the broker still holds.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::{BoundAddr, Runtime};
use ulo_rpc::__private::ClientProbe;
use ulo_rpc::{Link, Pattern};
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc_rabbitmq::RabbitMq;

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
