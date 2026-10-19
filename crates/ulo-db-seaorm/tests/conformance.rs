//! The store suite over `ulo-db-seaorm`: Postgres and MySQL each in a container per scenario behind a
//! relay (feature `integration`, a Docker daemon), SQLite on a file in a directory of its own.

#![cfg(any(feature = "sqlite", all(feature = "integration", any(feature = "postgres", feature = "mysql"))))]

use std::sync::Arc;

use sea_orm::{ConnectionTrait, DatabaseConnection, Statement};
use sea_orm_migration::prelude::*;
use serde_json::json;
use ulo::BoxFuture;
use ulo_db::StoreModule;
use ulo_db_conformance::MigrationProbe;
use ulo_db_seaorm::{SeaOrm, SeaOrmMigrations, SeaOrmOptions};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(dead_code, unused_imports)]
mod server;

use server::{CREATE_TABLE, CREATE_TABLE_HELD_SQLITE, Counted};

struct First;

impl MigrationName for First {
    fn name(&self) -> &str {
        "m20260101_000001_conformance"
    }
}

#[sea_orm_migration::async_trait::async_trait]
impl MigrationTrait for First {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let sql = match manager.get_database_backend() {
            sea_orm::DatabaseBackend::Sqlite => CREATE_TABLE_HELD_SQLITE,
            _ => CREATE_TABLE,
        };
        manager.get_connection().execute_unprepared(sql).await.map(drop)
    }
}

struct Migrator;

impl MigratorTrait for Migrator {
    fn migrations() -> Vec<Box<dyn MigrationTrait>> {
        vec![Box::new(First)]
    }
}

fn count(db: &DatabaseConnection) -> BoxFuture<'static, usize> {
    let db = db.clone();
    Box::pin(async move {
        let statement = Statement::from_string(db.get_database_backend(), "select count(*) from seaql_migrations");
        match db.query_one_raw(statement).await {
            Ok(Some(row)) => row.try_get_by_index::<i64>(0).map_or(0, |n| n as usize),
            _ => 0,
        }
    })
}

fn counted(probe: MigrationProbe) -> Counted<SeaOrm> {
    Counted { inner: Arc::new(SeaOrmMigrations::<Migrator>::new()), probe, count }
}

fn migrating(module: StoreModule<SeaOrm>, probe: MigrationProbe) -> StoreModule<SeaOrm> {
    module.migrate(counted(probe))
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use super::*;
    use ulo_db_conformance::Capabilities;

    struct SqliteHost {
        dir: server::Scratch,
        file: std::path::PathBuf,
        away: std::path::PathBuf,
    }

    impl SqliteHost {
        fn at(path: &std::path::Path) -> SeaOrmOptions {
            server::options(json!({ "url": format!("sqlite://{}", path.display()), "max_connections": 5 }))
        }
    }

    impl ulo_db_conformance::Host for SqliteHost {
        type Harness = ulo_db_conformance::OnTokio;
        type Store = SeaOrm;

        async fn start() -> Self {
            let dir = server::Scratch::new("seaorm-sqlite");
            let file = dir.0.join("db.sqlite");
            std::fs::File::create(&file).expect("an empty database file");
            let away = dir.0.join("db.sqlite.away");
            SqliteHost { dir, file, away }
        }

        fn options(&self) -> SeaOrmOptions {
            Self::at(&self.file)
        }

        fn unreachable_options(&self) -> SeaOrmOptions {
            Self::at(&self.dir.0.join("missing").join("db.sqlite"))
        }

        fn bad_credentials(&self) -> SeaOrmOptions {
            self.unreachable_options()
        }

        fn malformed_options(&self, password: &str) -> SeaOrmOptions {
            server::options(json!({ "url": format!("notsqlite://{password}@host/db") }))
        }

        async fn cut(&self) {
            std::fs::rename(&self.file, &self.away).expect("the database file moves away");
        }

        async fn restore(&self) {
            if self.away.exists() {
                std::fs::rename(&self.away, &self.file).expect("the database file moves back");
            }
        }

        fn lazily(&self, module: StoreModule<SeaOrm>) -> StoreModule<SeaOrm> {
            module.lazy()
        }

        fn migrating(&self, module: StoreModule<SeaOrm>, run: MigrationProbe) -> StoreModule<SeaOrm> {
            migrating(module, run)
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
    }

    ulo_db_conformance::conformance_suite!(SqliteHost; not_applicable {
        unreachable_database_fails_at_the_bound: "a file is not reached over a network",
        bad_credentials_fail_at_once: "a file has no credentials",
        a_connecting_create_is_retried_and_stops_on_permanent: "SeaORM's `create` connects nothing on this store",
        health_goes_down_in_an_outage_and_recovers: "a connection to a file stays open when the file is unlinked",
    });
}

macro_rules! server_host {
    ($module:ident, $feature:literal, $recipe:expr, $url:literal, $malformed:literal) => {
        #[cfg(all(feature = "integration", feature = $feature))]
        mod $module {
            use super::*;

            struct Host {
                server: server::Server,
            }

            impl Host {
                fn at(&self, password: &str, port: u16) -> SeaOrmOptions {
                    server::options(json!({ "url": format!($url, password, port), "max_connections": 5 }))
                }
            }

            impl ulo_db_conformance::Host for Host {
                type Harness = ulo_db_conformance::OnTokio;
                type Store = SeaOrm;

                const PARALLEL: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

                async fn start() -> Self {
                    Host { server: server::Server::start(&$recipe).await }
                }

                fn options(&self) -> SeaOrmOptions {
                    self.at(server::REAL_PASSWORD, self.server.relay.addr().port())
                }

                fn unreachable_options(&self) -> SeaOrmOptions {
                    // A second per attempt, so the three-second bound holds several.
                    server::options(json!({
                        "url": format!($url, server::REAL_PASSWORD, server::dead_port()),
                        "max_connections": 5,
                        "acquire_timeout_secs": 1,
                    }))
                }

                fn bad_credentials(&self) -> SeaOrmOptions {
                    self.at(ulo_db_conformance::PASSWORD, self.server.relay.addr().port())
                }

                fn malformed_options(&self, password: &str) -> SeaOrmOptions {
                    server::options(json!({ "url": format!($malformed, password) }))
                }

                async fn cut(&self) {
                    self.server.cut().await;
                }

                async fn restore(&self) {
                    self.server.restore().await;
                }

                fn lazily(&self, module: StoreModule<SeaOrm>) -> StoreModule<SeaOrm> {
                    module.lazy()
                }

                fn migrating(&self, module: StoreModule<SeaOrm>, run: MigrationProbe) -> StoreModule<SeaOrm> {
                    migrating(module, run)
                }
            }

            ulo_db_conformance::conformance_suite!(Host; not_applicable {
                a_connecting_create_is_retried_and_stops_on_permanent: "SeaORM's `create` connects nothing on this store",
            });
        }
    };
}

server_host!(
    postgres,
    "postgres",
    server::POSTGRES,
    "postgres://postgres:{}@127.0.0.1:{}/postgres",
    "postgres://postgres:{}@127.0.0.1:notaport/postgres"
);

server_host!(
    mysql,
    "mysql",
    server::MYSQL,
    "mysql://root:{}@127.0.0.1:{}/app",
    "mysql://root:{}@127.0.0.1:notaport/app"
);
