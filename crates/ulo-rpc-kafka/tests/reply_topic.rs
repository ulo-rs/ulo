//! A client named with `with_reply_topic` leaves one reply topic on the cluster however often it
//! restarts; an unnamed one leaves one per start. The partition knob shapes the reply topic it
//! creates.
//!
//! Needs a live Kafka from testcontainers, so it is gated behind the `integration` feature.
#![cfg(feature = "integration")]

use std::time::Duration;

use rdkafka::ClientConfig;
use rdkafka::consumer::{BaseConsumer, Consumer};
use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::kafka::apache::{KAFKA_PORT, Kafka};
use ulo::rpc::RpcClient;
use ulo_rpc_kafka::KafkaClientTransport;

async fn kafka() -> (ContainerAsync<Kafka>, String) {
    let container = Kafka::default()
        .start()
        .await
        .expect("a kafka container starts");
    let port = container
        .get_host_port_ipv4(KAFKA_PORT)
        .await
        .expect("kafka publishes its port");
    (container, format!("127.0.0.1:{port}"))
}

/// Every topic on the cluster, with its partition count.
fn topics(brokers: &str) -> Vec<(String, usize)> {
    let consumer: BaseConsumer = ClientConfig::new()
        .set("bootstrap.servers", brokers)
        .create()
        .expect("a metadata consumer");
    let metadata = consumer
        .fetch_metadata(None, Duration::from_secs(10))
        .expect("cluster metadata");
    metadata
        .topics()
        .iter()
        .map(|t| (t.name().to_string(), t.partitions().len()))
        .collect()
}

/// Three starts of the same logical client, each connecting and then dropping its transport,
/// the way a restarting process does.
async fn start_three_times(brokers: &str, build: impl Fn() -> KafkaClientTransport) {
    for _ in 0..3 {
        let client = RpcClient::new(build());
        client
            .connect()
            .await
            .expect("the client creates its reply topic and group");
        drop(client);
    }
}

#[tokio::test]
async fn a_named_client_reuses_one_reply_topic_across_restarts() {
    let (_container, brokers) = kafka().await;

    start_three_times(&brokers, || {
        KafkaClientTransport::new(brokers.clone())
            .with_reply_topic("orders-svc.replies")
            .with_topic_partitions(3)
    })
    .await;

    let topics = topics(&brokers);
    let named: Vec<_> = topics
        .iter()
        .filter(|(name, _)| name == "orders-svc.replies")
        .collect();
    assert_eq!(
        named.len(),
        1,
        "three starts of one named client left {} reply topics: {topics:?}",
        named.len()
    );
    assert_eq!(
        named[0].1, 3,
        "the reply topic was created with {} partitions, not the 3 asked for",
        named[0].1
    );
    assert!(
        !topics
            .iter()
            .any(|(name, _)| name.starts_with("ulo.rpc.reply.")),
        "a named client also created a per-transport reply topic: {topics:?}"
    );
}

/// The control: an unnamed client leaves a topic per start, which is what the name exists to
/// stop.
#[tokio::test]
async fn an_unnamed_client_leaves_a_reply_topic_per_start() {
    let (_container, brokers) = kafka().await;

    start_three_times(&brokers, || KafkaClientTransport::new(brokers.clone())).await;

    let per_transport = topics(&brokers)
        .into_iter()
        .filter(|(name, _)| name.starts_with("ulo.rpc.reply."))
        .count();
    assert_eq!(
        per_transport, 3,
        "three starts of an unnamed client left {per_transport} reply topics"
    );
}
