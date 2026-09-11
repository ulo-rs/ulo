//! The shared RPC conformance suite, against a live Kafka broker
//! (testcontainers).
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the broker.
//! Gated behind the `integration` feature because it needs Docker and
//! librdkafka.
#![cfg(feature = "integration")]

use std::time::Duration;

use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::kafka::apache::{KAFKA_PORT, Kafka};
use ulo_rpc_conformance::{Broker, Budget};
use ulo_rpc_kafka::{KafkaAdapter, KafkaClientTransport};

struct KafkaBroker {
    container: ContainerAsync<Kafka>,
    brokers: String,
}

impl Broker for KafkaBroker {
    type Adapter = KafkaAdapter;
    type Transport = KafkaClientTransport;

    async fn start() -> Self {
        let container = Kafka::default()
            .start()
            .await
            .expect("a kafka container starts");
        let port = container
            .get_host_port_ipv4(KAFKA_PORT)
            .await
            .expect("kafka publishes its port");
        Self {
            brokers: format!("127.0.0.1:{port}"),
            container,
        }
    }

    fn adapter(&self) -> Self::Adapter {
        KafkaAdapter::new(self.brokers.clone())
    }

    fn transport(&self) -> Self::Transport {
        KafkaClientTransport::new(self.brokers.clone()).with_timeout(Duration::from_secs(10))
    }

    /// Freeze the broker past the consumer session timeout so the group is
    /// declared dead, then thaw it. Recovery means rejoining the group and
    /// being reassigned the partitions, which is slower than any other
    /// transport here reconnecting.
    async fn disrupt(&self) {
        self.container.pause().await.expect("the container pauses");
        tokio::time::sleep(Duration::from_secs(15)).await;
        self.container.unpause().await.expect("the container thaws");
    }

    /// Kafka boots slowly, and a consumer joining a group waits out a
    /// rebalance before the first request is consumed. Every budget here is
    /// several times the default.
    fn budget() -> Budget {
        Budget {
            boot: Duration::from_secs(90),
            settle: Duration::from_secs(30),
            recovery: Duration::from_secs(120),
        }
    }
}

ulo_rpc_conformance::conformance_suite!(KafkaBroker);
