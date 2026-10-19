//! The store suite over `ulo-db-sqlx`: Postgres and MySQL each in a container per scenario behind a
//! relay (feature `integration`, a Docker daemon), SQLite on a file in a directory of its own.

#![cfg(any(feature = "sqlite", all(feature = "integration", any(feature = "postgres", feature = "mysql"))))]

use std::borrow::Cow;
use std::sync::Arc;

use serde_json::json;
use sqlx::migrate::{Migration, MigrationType, Migrator};
use ulo::BoxFuture;
use ulo_db::StoreModule;
use ulo_db_conformance::{Capabilities, MigrationProbe};
use ulo_db_sqlx::sqlx;

#[path = "../../ulo-db-conformance/support/server.rs"]
#[allow(dead_code, unused_imports)]
mod server;

use server::{CREATE_TABLE, Counted};

fn migrator() -> Migrator {
    Migrator {
        migrations: Cow::Owned(vec![Migration::new(
            1,
            "conformance".into(),
            MigrationType::Simple,
            sqlx::SqlSafeStr::into_sql_str(CREATE_TABLE),
            false,
        )]),
        ..Migrator::DEFAULT
    }
}

macro_rules! counting {
    ($store:ty, $pool:ty) => {
        fn count(pool: &$pool) -> BoxFuture<'static, usize> {
            let pool = pool.clone();
            Box::pin(async move {
                sqlx::query_scalar::<_, i64>("select count(*) from _sqlx_migrations")
                    .fetch_one(&pool)
                    .await
                    .map_or(0, |n| n as usize)
            })
        }

        fn counted(probe: MigrationProbe) -> Counted<$store> {
            Counted { inner: Arc::new(migrator()), probe, count }
        }
    };
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use ulo_db_sqlx::{Sqlite, SqliteOptions};

    static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

    counting!(Sqlite, sqlx::SqlitePool);

    struct SqliteHost {
        dir: PathBuf,
        file: PathBuf,
        away: PathBuf,
    }

    impl SqliteHost {
        fn url(path: &std::path::Path) -> serde_json::Value {
            json!({ "url": format!("sqlite://{}", path.display()), "max_connections": 5 })
        }
    }

    impl Drop for SqliteHost {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl ulo_db_conformance::Host for SqliteHost {
        type Harness = ulo_db_conformance::OnTokio;
        type Store = Sqlite;

        async fn start() -> Self {
            let dir = std::env::temp_dir().join(format!("ulo-conf-sqlite-{}-{}", std::process::id(), SEQUENCE.fetch_add(1, Ordering::SeqCst)));
            std::fs::create_dir_all(&dir).expect("a directory for the database file");
            let file = dir.join("db.sqlite");
            std::fs::File::create(&file).expect("an empty database file");
            let away = dir.join("db.sqlite.away");
            SqliteHost { dir, file, away }
        }

        fn options(&self) -> SqliteOptions {
            server::options(Self::url(&self.file))
        }

        fn unreachable_options(&self) -> SqliteOptions {
            server::options(Self::url(&self.dir.join("missing").join("db.sqlite")))
        }

        fn bad_credentials(&self) -> SqliteOptions {
            self.unreachable_options()
        }

        fn malformed_options(&self, password: &str) -> SqliteOptions {
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

        fn lazily(&self, module: StoreModule<Sqlite>) -> StoreModule<Sqlite> {
            module.lazy()
        }

        fn migrating(&self, module: StoreModule<Sqlite>, run: MigrationProbe) -> StoreModule<Sqlite> {
            module.migrate(counted(run))
        }

        fn capabilities(&self) -> Capabilities {
            Capabilities::default()
        }
    }

    ulo_db_conformance::conformance_suite!(SqliteHost; not_applicable {
        unreachable_database_fails_at_the_bound: "a file is not reached over a network",
        bad_credentials_fail_at_once: "a file has no credentials",
        a_connecting_create_is_retried_and_stops_on_permanent: "SQLite's `create` connects nothing",
        health_goes_down_in_an_outage_and_recovers: "a connection to a file stays open when the file is unlinked",
    });
}

macro_rules! server_host {
    ($module:ident, $feature:literal, $store:ident, $options:ident, $pool:ty, $recipe:expr, $url:literal, $user_url:literal) => {
        #[cfg(all(feature = "integration", feature = $feature))]
        mod $module {
            use super::*;
            use ulo_db_sqlx::{$options, $store};

            counting!($store, $pool);

            struct Host {
                server: server::Server,
            }

            impl Host {
                fn at(&self, password: &str, port: u16) -> $options {
                    server::options(json!({ "url": format!($url, password, port), "max_connections": 5 }))
                }
            }

            impl ulo_db_conformance::Host for Host {
                type Harness = ulo_db_conformance::OnTokio;
                type Store = $store;

                const PARALLEL: Option<std::num::NonZeroUsize> = std::num::NonZeroUsize::new(4);

                async fn start() -> Self {
                    Host { server: server::Server::start(&$recipe).await }
                }

                fn options(&self) -> $options {
                    self.at(server::REAL_PASSWORD, self.server.relay.addr().port())
                }

                fn unreachable_options(&self) -> $options {
                    // A second per attempt, so the three-second bound holds several.
                    server::options(json!({
                        "url": format!($url, server::REAL_PASSWORD, server::dead_port()),
                        "max_connections": 5,
                        "acquire_timeout_secs": 1,
                    }))
                }

                fn bad_credentials(&self) -> $options {
                    self.at(ulo_db_conformance::PASSWORD, self.server.relay.addr().port())
                }

                fn malformed_options(&self, password: &str) -> $options {
                    server::options(json!({ "url": format!($user_url, password) }))
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
                    module.migrate(counted(run))
                }
            }

            ulo_db_conformance::conformance_suite!(Host; not_applicable {
                a_connecting_create_is_retried_and_stops_on_permanent: "SQLx's pool connects lazily, so `create` connects nothing",
            });
        }
    };
}

server_host!(
    postgres,
    "postgres",
    Postgres,
    PgOptions,
    sqlx::PgPool,
    server::POSTGRES,
    "postgres://postgres:{}@127.0.0.1:{}/postgres",
    "postgres://postgres:{}@127.0.0.1:notaport/postgres"
);

server_host!(
    mysql,
    "mysql",
    MySql,
    MySqlOptions,
    sqlx::MySqlPool,
    server::MYSQL,
    "mysql://root:{}@127.0.0.1:{}/app",
    "mysql://root:{}@127.0.0.1:notaport/app"
);
