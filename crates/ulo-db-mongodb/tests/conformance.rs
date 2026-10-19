//! The store suite over `ulo-db-mongodb`, each scenario against a server in a container of its own, the
//! client reaching it through a relay that `cut` severs.

#![cfg(feature = "integration")]

use serde_json::json;
use ulo_db::StoreModule;
use ulo_db_mongodb::{Mongo, MongoOptions};
use ulo_db_conformance::MigrationProbe;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
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
        module.lazy()
    }

    fn migrating(&self, module: StoreModule<Mongo>, _run: MigrationProbe) -> StoreModule<Mongo> {
        module
    }

    fn capabilities(&self) -> ulo_db_conformance::Capabilities {
        ulo_db_conformance::Capabilities::default().migrations(false)
    }
}

ulo_db_conformance::conformance_suite!(Host; not_applicable {
    a_connecting_create_is_retried_and_stops_on_permanent: "the host's `mongodb://` URI resolves nothing in `create`; only a `mongodb+srv://` URI connects there, and no DNS exists in the harness",
    migrations_run_only_on_request: "MongoDB has no migrator",
    migrations_run_once_across_replicas: "MongoDB has no migrator",
});
