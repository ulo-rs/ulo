//! The store suite over `ulo-db-redis`, each scenario against a server in a container of its own, the
//! client reaching it through a relay that `cut` severs.

#![cfg(feature = "integration")]

use serde_json::json;
use ulo_db::StoreModule;
use ulo_db_redis::{Redis, RedisOptions};
use ulo_db_conformance::MigrationProbe;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(dead_code, unused_imports)]
mod server;

struct Host {
    server: server::Server,
}

impl Host {
    fn at(&self, password: &str, port: u16) -> RedisOptions {
        server::options(json!({ "url": format!("redis://:{}@127.0.0.1:{}", password, port) }))
    }
}

impl ulo_db_conformance::Host for Host {
    type Harness = ulo_db_conformance::OnTokio;
    type Store = Redis;

    const PARALLEL: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

    async fn start() -> Self {
        Host { server: server::Server::start(&server::REDIS).await }
    }

    fn options(&self) -> RedisOptions {
        self.at(server::REAL_PASSWORD, self.server.relay.addr().port())
    }

    fn unreachable_options(&self) -> RedisOptions {
        // A second per attempt, so the three-second bound holds several.
        server::options(json!({
            "url": format!("redis://:{}@127.0.0.1:{}", server::REAL_PASSWORD, server::dead_port()),
            "connection_timeout_secs": 1,
        }))
    }

    fn bad_credentials(&self) -> RedisOptions {
        self.at(ulo_db_conformance::PASSWORD, self.server.relay.addr().port())
    }

    fn malformed_options(&self, password: &str) -> RedisOptions {
        server::options(json!({ "url": format!("redis://:{}@127.0.0.1:notaport", password) }))
    }

    async fn cut(&self) {
        self.server.cut().await;
    }

    async fn restore(&self) {
        self.server.restore().await;
    }

    fn lazily(&self, module: StoreModule<Redis>) -> StoreModule<Redis> {
        module
    }

    fn migrating(&self, module: StoreModule<Redis>, _run: MigrationProbe) -> StoreModule<Redis> {
        module
    }

    fn capabilities(&self) -> ulo_db_conformance::Capabilities {
        ulo_db_conformance::Capabilities::default().lazy(false).migrations(false)
    }
}

ulo_db_conformance::conformance_suite!(Host; not_applicable {
    lazy_readiness_waits_for_the_first_ping: "`create` connects, so the store offers no `.lazy()`",
    migrations_run_only_on_request: "Redis has no migrator",
    migrations_run_once_across_replicas: "Redis has no migrator",
});
