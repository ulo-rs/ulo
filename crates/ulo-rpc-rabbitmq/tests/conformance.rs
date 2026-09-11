//! The shared RPC conformance suite, against a live RabbitMQ
//! (testcontainers).
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the broker.
//! Gated behind the `integration` feature because it needs Docker.
#![cfg(feature = "integration")]

use std::time::Duration;

use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::rabbitmq::RabbitMq;
use ulo_rpc_conformance::Broker;
use ulo_rpc_rabbitmq::{RabbitMqAdapter, RabbitMqClientTransport};

struct RabbitMqBroker {
    container: ContainerAsync<RabbitMq>,
    uri: String,
}

impl Broker for RabbitMqBroker {
    type Adapter = RabbitMqAdapter;
    type Transport = RabbitMqClientTransport;

    async fn start() -> Self {
        let container = RabbitMq::default()
            .start()
            .await
            .expect("a rabbitmq container starts");
        let port = container
            .get_host_port_ipv4(5672)
            .await
            .expect("rabbitmq publishes its port");
        Self {
            uri: format!("amqp://guest:guest@127.0.0.1:{port}/%2f"),
            container,
        }
    }

    fn adapter(&self) -> Self::Adapter {
        RabbitMqAdapter::new(self.uri.clone())
    }

    fn transport(&self) -> Self::Transport {
        RabbitMqClientTransport::new(self.uri.clone()).with_timeout(Duration::from_secs(2))
    }

    /// Freeze the broker long enough for AMQP heartbeats to lapse on both
    /// ends, then thaw it. lapin's auto-recovery re-declares the queues and
    /// re-establishes the direct reply-to consumer.
    async fn disrupt(&self) {
        self.container.pause().await.expect("the container pauses");
        tokio::time::sleep(Duration::from_secs(10)).await;
        self.container.unpause().await.expect("the container thaws");
    }
}

ulo_rpc_conformance::conformance_suite!(RabbitMqBroker);
