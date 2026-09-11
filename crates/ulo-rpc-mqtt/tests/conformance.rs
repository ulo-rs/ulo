//! The shared RPC conformance suite, against a live Mosquitto broker
//! (testcontainers).
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the broker.
//! Gated behind the `integration` feature because it needs Docker.
#![cfg(feature = "integration")]

use std::time::Duration;

use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mosquitto::Mosquitto;
use ulo_rpc_conformance::Broker;
use ulo_rpc_mqtt::{MqttAdapter, MqttClientTransport};

struct MqttBroker {
    container: ContainerAsync<Mosquitto>,
    host: String,
    port: u16,
}

impl Broker for MqttBroker {
    type Adapter = MqttAdapter;
    type Transport = MqttClientTransport;

    async fn start() -> Self {
        let container = Mosquitto::default()
            .start()
            .await
            .expect("a mosquitto container starts");
        let host = container
            .get_host()
            .await
            .expect("mosquitto reports its host")
            .to_string();
        let port = container
            .get_host_port_ipv4(1883)
            .await
            .expect("mosquitto publishes its port");
        Self {
            container,
            host,
            port,
        }
    }

    fn adapter(&self) -> Self::Adapter {
        MqttAdapter::new(self.host.clone(), self.port)
    }

    fn transport(&self) -> Self::Transport {
        MqttClientTransport::new(self.host.clone(), self.port).with_timeout(Duration::from_secs(2))
    }

    /// Freeze the broker past the 5s keepalive on both ends, which is what
    /// makes each side declare the connection dead, then thaw it. Both have to
    /// re-subscribe on the next ConnAck.
    async fn disrupt(&self) {
        self.container.pause().await.expect("the container pauses");
        tokio::time::sleep(Duration::from_secs(10)).await;
        self.container.unpause().await.expect("the container thaws");
    }
}

ulo_rpc_conformance::conformance_suite!(MqttBroker);
