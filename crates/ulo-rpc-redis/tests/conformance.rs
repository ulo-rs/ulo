//! The RPC conformance suite over the Redis link, each scenario against a Redis server in a
//! container of its own, since Pub/Sub channels span every database of a server. The client
//! reaches it through a relay that `disrupt` cuts.
//!
//! Beside the suite, a test drives the link through `Link` directly, for an ordering no scenario
//! reaches on its own: a request read after the drain ended the inbound stream. Two more drop each
//! side of the link without `close` and count, through a relay, the connections Redis still holds.

#![cfg(feature = "integration")]

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use ulo::{BoundAddr, Runtime};
use ulo_rpc_conformance::{Broker, report, startup_failed};
use ulo_rpc_conformance::relay::{Relay, reachable, unshadowed};
use ulo_rpc::{CallHeaders, Data, Frame, Link, Pattern};
use ulo_rpc_redis::Redis;
use ulo_transport::ErrorKind;

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
    let mut inbound = server.listen(&[Pattern::from(DRAINED)]).await.expect("the server's link listens");
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
    let inbound = server.listen(&[Pattern::from("link.dropped")]).await.expect("the server's link listens");
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
