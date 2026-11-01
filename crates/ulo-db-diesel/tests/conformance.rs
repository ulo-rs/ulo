//! The store suite over `ulo-db-diesel`: Postgres and MySQL each in a container per scenario behind a
//! relay (feature `integration`, a Docker daemon), SQLite on a file in a directory of its own.

#![cfg(any(feature = "sqlite", all(feature = "integration", any(feature = "postgres", feature = "mysql"))))]

use std::path::Path;
use std::sync::Arc;

use diesel_migrations::FileBasedMigrations;
use serde_json::json;
use ulo::BoxFuture;
use ulo_db::StoreModule;
use ulo_db_conformance::MigrationProbe;
use ulo_db_diesel::{DieselMigrations, DieselOptions};

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(unused_imports)]
mod server;

use server::{CREATE_TABLE, Counted};

#[derive(diesel::QueryableByName)]
struct Count {
    #[diesel(sql_type = diesel::sql_types::BigInt)]
    n: i64,
}

/// A migrations directory holding the one migration every host applies.
fn migrations_in(dir: &Path) -> FileBasedMigrations {
    let migration = dir.join("2026-01-01-000001_conformance");
    std::fs::create_dir_all(&migration).expect("a migration directory");
    std::fs::write(migration.join("up.sql"), CREATE_TABLE).expect("up.sql");
    std::fs::write(migration.join("down.sql"), "drop table ulo_conf_applied").expect("down.sql");
    FileBasedMigrations::from_path(dir).expect("the migrations directory reads")
}

macro_rules! counting {
    ($store:ty, $pool:ty) => {
        fn count(pool: &$pool) -> BoxFuture<'static, usize> {
            let pool = pool.clone();
            Box::pin(async move {
                use diesel_async::RunQueryDsl;
                let Ok(mut conn) = pool.get().await else { return 0 };
                diesel::sql_query("select count(*) as n from __diesel_schema_migrations")
                    .get_result::<Count>(&mut *conn)
                    .await
                    .map_or(0, |row| row.n as usize)
            })
        }

        fn migrating(dir: &Path, module: StoreModule<$store>, probe: MigrationProbe) -> StoreModule<$store> {
            module.migrate(Counted::<$store> {
                inner: Arc::new(DieselMigrations::new(migrations_in(dir))),
                probe,
                count,
            })
        }
    };
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use super::*;
    use ulo_db_diesel::{Sqlite, SqlitePool};

    counting!(Sqlite, SqlitePool);

    struct SqliteHost {
        dir: server::Scratch,
        file: std::path::PathBuf,
        away: std::path::PathBuf,
    }

    impl SqliteHost {
        fn at(path: &Path) -> DieselOptions {
            server::options(json!({ "url": path.display().to_string(), "max_size": 5 }))
        }
    }

    impl ulo_db_conformance::Host for SqliteHost {
        type Harness = ulo_db_conformance::OnTokio;
        type Store = Sqlite;

        async fn start() -> Self {
            let dir = server::Scratch::new("diesel-sqlite");
            let file = dir.0.join("db.sqlite");
            std::fs::File::create(&file).expect("an empty database file");
            let away = dir.0.join("db.sqlite.away");
            SqliteHost { dir, file, away }
        }

        fn options(&self) -> DieselOptions {
            Self::at(&self.file)
        }

        fn unreachable_options(&self) -> DieselOptions {
            Self::at(&self.dir.0.join("missing").join("db.sqlite"))
        }

        fn bad_credentials(&self) -> DieselOptions {
            self.unreachable_options()
        }

        fn malformed_options(&self, password: &str) -> DieselOptions {
            server::options(json!({ "url": format!("notsqlite://{password}@host/db") }))
        }

        // Diesel creates a missing file, so the outage puts a directory where the file was.
        async fn cut(&self) {
            std::fs::rename(&self.file, &self.away).expect("the database file moves away");
            std::fs::create_dir(&self.file).expect("a directory stands where the file was");
        }

        async fn restore(&self) {
            if self.away.exists() {
                std::fs::remove_dir(&self.file).expect("the stand-in directory goes");
                std::fs::rename(&self.away, &self.file).expect("the database file moves back");
            }
        }

        fn lazily(&self, module: StoreModule<Sqlite>) -> StoreModule<Sqlite> {
            module.lazy()
        }

        fn migrating(&self, module: StoreModule<Sqlite>, run: MigrationProbe) -> StoreModule<Sqlite> {
            migrating(&self.dir.0.join("migrations"), module, run)
        }
    }

    ulo_db_conformance::conformance_suite!(SqliteHost; not_applicable {
        unreachable_database_fails_at_the_bound: "a file is not reached over a network",
        bad_credentials_fail_at_once: "a file has no credentials",
        a_connecting_create_is_retried_and_stops_on_permanent: "the pool connects nothing in `create`",
        health_goes_down_in_an_outage_and_recovers: "a connection to a file stays open when the file is unlinked",
    });
}

macro_rules! server_host {
    ($module:ident, $feature:literal, $store:ident, $pool:ident, $recipe:expr, $url:literal, $malformed:literal) => {
        #[cfg(all(feature = "integration", feature = $feature))]
        mod $module {
            use super::*;
            use ulo_db_diesel::{$pool, $store};

            counting!($store, $pool);

            struct Host {
                server: server::Server,
                migrations: server::Scratch,
            }

            impl Host {
                fn at(&self, password: &str, port: u16) -> DieselOptions {
                    server::options(json!({ "url": format!($url, password, port), "max_size": 5 }))
                }
            }

            impl ulo_db_conformance::Host for Host {
                type Harness = ulo_db_conformance::OnTokio;
                type Store = $store;

                const PARALLEL: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

                async fn start() -> Self {
                    Host { server: server::Server::start(&$recipe).await, migrations: server::Scratch::new("diesel-migrations") }
                }

                fn options(&self) -> DieselOptions {
                    self.at(server::REAL_PASSWORD, self.server.relay.addr().port())
                }

                fn unreachable_options(&self) -> DieselOptions {
                    self.at(server::REAL_PASSWORD, server::dead_port())
                }

                fn bad_credentials(&self) -> DieselOptions {
                    self.at(ulo_db_conformance::PASSWORD, self.server.relay.addr().port())
                }

                fn malformed_options(&self, password: &str) -> DieselOptions {
                    server::options(json!({ "url": format!($malformed, password) }))
                }

                async fn cut(&self) {
                    self.server.cut().await;
                }

                async fn restore(&self) {
                    self.server.restore().await;
                }

                fn lazily(&self, module: StoreModule<$store>) -> StoreModule<$store> {
                    module.lazy()
                }

                fn migrating(&self, module: StoreModule<$store>, run: MigrationProbe) -> StoreModule<$store> {
                    migrating(&self.migrations.0, module, run)
                }
            }

            ulo_db_conformance::conformance_suite!(Host; not_applicable {
                a_connecting_create_is_retried_and_stops_on_permanent: "the pool connects nothing in `create`",
            });
        }
    };
}

server_host!(
    postgres,
    "postgres",
    Postgres,
    PgPool,
    server::POSTGRES,
    "postgres://postgres:{}@127.0.0.1:{}/postgres",
    "postgres://postgres:{}@127.0.0.1:notaport/postgres"
);

server_host!(
    mysql,
    "mysql",
    MySql,
    MySqlPool,
    server::MYSQL,
    "mysql://root:{}@127.0.0.1:{}/app",
    "mysql://root:{}@127.0.0.1:notaport/app"
);
