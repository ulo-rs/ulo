//! The shared RPC conformance suite, against a live NATS server
//! (testcontainers).
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the broker.
//! Gated behind the `integration` feature because it needs Docker.
#![cfg(feature = "integration")]

use std::time::Duration;

use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::nats::Nats;
use ulo_rpc_conformance::Broker;
use ulo_rpc_nats::{NatsAdapter, NatsClientTransport};

struct NatsBroker {
    container: ContainerAsync<Nats>,
    endpoint: String,
}

impl Broker for NatsBroker {
    type Adapter = NatsAdapter;
    type Transport = NatsClientTransport;

    async fn start() -> Self {
        let container = Nats::default()
            .start()
            .await
            .expect("a nats container starts");
        let port = container
            .get_host_port_ipv4(4222)
            .await
            .expect("nats publishes its port");
        Self {
            endpoint: format!("nats://127.0.0.1:{port}"),
            container,
        }
    }

    fn adapter(&self) -> Self::Adapter {
        NatsAdapter::new(self.endpoint.clone())
    }

    fn transport(&self) -> Self::Transport {
        NatsClientTransport::new(self.endpoint.clone()).with_timeout(Duration::from_secs(2))
    }

    /// Freeze the server past the client's ping interval so both ends declare
    /// the connection dead, then thaw it for `async-nats` to reconnect and
    /// re-subscribe.
    async fn disrupt(&self) {
        self.container.pause().await.expect("the container pauses");
        tokio::time::sleep(Duration::from_secs(10)).await;
        self.container.unpause().await.expect("the container thaws");
    }
}

ulo_rpc_conformance::conformance_suite!(NatsBroker);
