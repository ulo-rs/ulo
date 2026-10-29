//! The same handlers over each broker, in a container of its own. The tests need a running Docker
//! daemon and are built by the `integration` feature.

#![cfg(feature = "integration")]

mod common;

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, ContainerRequest, GenericImage, ImageExt};
use ulo_conformance_support::relay::{reachable, unshadowed};
use ulo_conformance_support::{report, startup_failed};

/// Starts the container `image` builds under `name`, again if another listener shadows it, and returns the host address of `port`.
async fn start(name: &str, image: impl Fn() -> ContainerRequest<GenericImage>, port: u16) -> (ContainerAsync<GenericImage>, SocketAddr) {
    let (container, addrs) = unshadowed(name, || async {
        let container = image()
            .with_container_name(name)
            .start()
            .await
            .unwrap_or_else(|error| startup_failed!("{name} did not start: {}", report(&error)));
        let mapped = container.get_host_port_ipv4(port).await.unwrap_or_else(|error| startup_failed!("{name}'s port is not mapped: {}", report(&error)));
        (container, vec![SocketAddr::from((Ipv4Addr::LOCALHOST, mapped))])
    })
    .await;
    reachable(addrs[0], Duration::from_secs(10)).await;
    (container, addrs[0])
}

fn image(name: &str, tag: &str, port: u16, ready: WaitFor) -> ContainerRequest<GenericImage> {
    GenericImage::new(name, tag).with_exposed_port(port.tcp()).with_wait_for(ready).into()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_handlers_answer_over_nats() {
    let nats = || image("nats", "2.10.14", 4222, WaitFor::message_on_stderr("Server is ready"));
    let (_nats, addr) = start("ulo-conf-ex28-nats", nats, 4222).await;
    common::the_handlers_answer(&format!("nats://{addr}")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_handlers_answer_over_redis() {
    let redis = || image("redis", "7-alpine", 6379, WaitFor::message_on_stdout("Ready to accept connections"));
    let (_redis, addr) = start("ulo-conf-ex28-redis", redis, 6379).await;
    common::the_handlers_answer(&format!("redis://{addr}")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_handlers_answer_over_mqtt() {
    let mosquitto = || {
        image("eclipse-mosquitto", "2.0.18", 1883, WaitFor::message_on_stderr(" running")).with_cmd(["mosquitto", "-c", "/mosquitto-no-auth.conf"])
    };
    let (_mosquitto, addr) = start("ulo-conf-ex28-mqtt", mosquitto, 1883).await;
    common::the_handlers_answer(&format!("mqtt://{addr}")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_handlers_answer_over_rabbitmq() {
    let rabbit = || image("rabbitmq", "4.1-alpine", 5672, WaitFor::message_on_stdout("Server startup complete"));
    let (_rabbit, addr) = start("ulo-conf-ex28-rabbitmq", rabbit, 5672).await;
    common::the_handlers_answer(&format!("amqp://{addr}")).await;
}

/// A Kafka client dials what the broker advertises, so the container's port is published at the
/// host port it advertises.
#[tokio::test(flavor = "multi_thread")]
async fn the_handlers_answer_over_kafka() {
    const NAME: &str = "ulo-conf-ex28-kafka";
    let port = common::free_port();
    let kafka = GenericImage::new("apache/kafka-native", "3.8.0")
        .with_exposed_port(9092.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Kafka Server started"))
        .with_mapped_port(port, 9092.tcp())
        .with_container_name(NAME)
        .with_env_var("CLUSTER_ID", "ulo-ex28-kafka")
        .with_env_var("KAFKA_NODE_ID", "1")
        .with_env_var("KAFKA_PROCESS_ROLES", "broker,controller")
        .with_env_var("KAFKA_CONTROLLER_QUORUM_VOTERS", "1@localhost:9093")
        .with_env_var("KAFKA_CONTROLLER_LISTENER_NAMES", "CONTROLLER")
        .with_env_var("KAFKA_LISTENERS", "PLAINTEXT://:9092,CONTROLLER://:9093")
        .with_env_var("KAFKA_ADVERTISED_LISTENERS", format!("PLAINTEXT://localhost:{port}"))
        .with_env_var("KAFKA_LISTENER_SECURITY_PROTOCOL_MAP", "PLAINTEXT:PLAINTEXT,CONTROLLER:PLAINTEXT")
        .with_env_var("KAFKA_INTER_BROKER_LISTENER_NAME", "PLAINTEXT")
        .with_env_var("KAFKA_OFFSETS_TOPIC_REPLICATION_FACTOR", "1")
        .with_env_var("KAFKA_OFFSETS_TOPIC_NUM_PARTITIONS", "1")
        .with_env_var("KAFKA_TRANSACTION_STATE_LOG_REPLICATION_FACTOR", "1")
        .with_env_var("KAFKA_TRANSACTION_STATE_LOG_MIN_ISR", "1")
        .with_env_var("KAFKA_GROUP_INITIAL_REBALANCE_DELAY_MS", "0");
    let _kafka = kafka.start().await.unwrap_or_else(|error| startup_failed!("{NAME} did not start: {}", report(&error)));
    reachable(SocketAddr::from((Ipv4Addr::LOCALHOST, port)), Duration::from_secs(10)).await;
    common::the_handlers_answer(&format!("kafka://localhost:{port}")).await;
}
