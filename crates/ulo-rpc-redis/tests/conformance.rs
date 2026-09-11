//! The shared RPC conformance suite, against a live Redis (testcontainers).
//!
//! The cases live in `ulo-rpc-conformance`; this file supplies the broker.
//! Gated behind the `integration` feature because it needs Docker.
#![cfg(feature = "integration")]

use std::time::Duration;

use testcontainers::ContainerAsync;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;
use ulo_rpc_conformance::Broker;
use ulo_rpc_redis::{RedisAdapter, RedisClientTransport};

struct RedisBroker {
    // Dropped with the broker, which stops the container.
    _container: ContainerAsync<Redis>,
    endpoint: String,
}

impl Broker for RedisBroker {
    type Adapter = RedisAdapter;
    type Transport = RedisClientTransport;

    async fn start() -> Self {
        let container = Redis::default()
            .start()
            .await
            .expect("a redis container starts");
        let port = container
            .get_host_port_ipv4(6379)
            .await
            .expect("redis publishes its port");
        Self {
            endpoint: format!("redis://127.0.0.1:{port}"),
            _container: container,
        }
    }

    fn adapter(&self) -> Self::Adapter {
        RedisAdapter::new(self.endpoint.clone())
    }

    fn transport(&self) -> Self::Transport {
        RedisClientTransport::new(self.endpoint.clone()).with_timeout(Duration::from_secs(2))
    }

    /// Redis Pub/Sub carries no application-level heartbeat, so pausing the
    /// container is not noticed — the TCP socket stays open and both sides go
    /// on believing they are connected. `CLIENT KILL` from a separate control
    /// connection drops the pubsub and normal connections server-side, which
    /// is a disconnect the transport has to observe and recover from.
    ///
    /// `SKIPME` defaults to yes, so the control connection survives its own
    /// kill.
    async fn disrupt(&self) {
        let client = redis::Client::open(self.endpoint.as_str()).expect("a control client opens");
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("the control connection connects");
        for kind in ["pubsub", "normal"] {
            let _: i64 = redis::cmd("CLIENT")
                .arg("KILL")
                .arg("TYPE")
                .arg(kind)
                .query_async(&mut conn)
                .await
                .unwrap_or(0);
        }
    }
}

ulo_rpc_conformance::conformance_suite!(RedisBroker);
