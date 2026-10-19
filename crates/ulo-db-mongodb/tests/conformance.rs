//! The store suite over `ulo-db-mongodb`, each scenario against a server in a container of its own, the
//! client reaching it through a relay that `cut` severs.

#![cfg(feature = "integration")]

use serde_json::json;
use ulo_db::StoreModule;
use ulo_db_mongodb::{Mongo, MongoOptions};
use ulo_db_conformance::MigrationProbe;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(dead_code, unused_imports)]
mod server;

struct Host {
    server: server::Server,
}

impl Host {
    fn at(&self, port: u16) -> MongoOptions {
        server::options(json!({ "uri": format!("mongodb://127.0.0.1:{port}/") }))
    }
}

impl ulo_db_conformance::Host for Host {
    type Harness = ulo_db_conformance::OnTokio;
    type Store = Mongo;

    const PARALLEL: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

    async fn start() -> Self {
        Host { server: server::Server::start(&server::MONGO).await }
    }

    fn options(&self) -> MongoOptions {
        self.at(self.server.relay.addr().port())
    }

    fn unreachable_options(&self) -> MongoOptions {
        // A second per attempt, so the three-second bound holds several.
        server::options(json!({ "uri": format!("mongodb://127.0.0.1:{}/?serverSelectionTimeoutMS=1000", server::dead_port()) }))
    }

    fn bad_credentials(&self) -> MongoOptions {
        let port = self.server.relay.addr().port();
        server::options(json!({ "uri": format!("mongodb://root:{}@127.0.0.1:{port}/?authSource=admin", ulo_db_conformance::PASSWORD) }))
    }

    fn malformed_options(&self, password: &str) -> MongoOptions {
        server::options(json!({ "uri": format!("mongodb://root:{}@/?authSource=admin", password) }))
    }

    async fn cut(&self) {
        self.server.cut().await;
    }

    async fn restore(&self) {
        self.server.restore().await;
    }

    fn lazily(&self, module: StoreModule<Mongo>) -> StoreModule<Mongo> {
        module
    }

    fn migrating(&self, module: StoreModule<Mongo>, _run: MigrationProbe) -> StoreModule<Mongo> {
        module
    }

    fn capabilities(&self) -> ulo_db_conformance::Capabilities {
        ulo_db_conformance::Capabilities::default().lazy(false).migrations(false)
    }
}

ulo_db_conformance::conformance_suite!(Host; not_applicable {
    lazy_readiness_waits_for_the_first_ping: "`create` connects, so the store offers no `.lazy()`",
    migrations_run_only_on_request: "MongoDB has no migrator",
    migrations_run_once_across_replicas: "MongoDB has no migrator",
}; pending {
    unreachable_database_fails_at_the_bound: "pending the laziness batch, designer ruling QUESTIONS_4 Q2/Q5: `create` connects nothing on a plain mongodb:// URL and `ping` does not retry, so an unreachable server fails at the first attempt; that batch must un-ignore this scenario",
});
