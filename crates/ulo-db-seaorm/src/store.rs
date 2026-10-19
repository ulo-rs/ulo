use std::error::Error;
use std::fmt;
use std::time::Duration;

use sea_orm::{ConnectOptions, Database, DatabaseConnection, DbBackend, DbErr};
use ulo::{BoxError, BoxFuture, Permanent, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, Store};

use crate::options::{SeaOrmOptions, password_texts, scrub};

/// The SeaORM store; the client is `sea_orm::DatabaseConnection`.
///
/// Errors are classified through the SQLx error SeaORM carries, as `ulo-db-sqlx`'s table does.
pub struct SeaOrm;

impl Store for SeaOrm {
    const NAME: &'static str = "seaorm";
    type Options = SeaOrmOptions;
    type Client = DatabaseConnection;
    fn create_connects(_: &SeaOrmOptions) -> bool {
        false
    }

    fn validate(o: &SeaOrmOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URL's password component, beside the whole URL the section read registered.
    fn secrets(o: &SeaOrmOptions) -> Vec<Secret<String>> {
        password_texts(o.url.expose()).into_iter().map(Secret::new).collect()
    }

    /// Connects nothing. Postgres and MySQL: `ConnectOptions::connect_lazy(true)`. SQLite: the lazy
    /// pool of `ulo-db-sqlx`, wrapped by `SqlxSqliteConnector::from_sqlx_sqlite_pool`, because
    /// SeaORM's own SQLite connector runs `SELECT sqlite_version()` while connecting whenever
    /// `sqlite-use-returning-for-3_35` is on, and `sea-orm-migration` turns it on for every build.
    /// The three options the store exposes map onto the pool as documented on
    /// [`SeaOrmOptions`]; SeaORM's own `ConnectOptions` knobs are not exposed and do not apply.
    fn create(o: &SeaOrmOptions) -> BoxFuture<'static, Result<DatabaseConnection, BoxError>> {
        let o = o.clone();
        Box::pin(async move {
            let url = o.url.expose().clone();
            #[cfg(feature = "sqlite")]
            if crate::options::engine_of(&url) == Some(ulo_db_sqlx::Engine::Sqlite) {
                let pool = <ulo_db_sqlx::Sqlite as Store>::create(&ulo_db_sqlx::SqliteOptions {
                    url: o.url.clone(),
                    max_connections: o.max_connections.or(Some(SQLITE_DEFAULT_MAX_CONNECTIONS)),
                    min_connections: o.min_connections,
                    acquire_timeout_secs: o.acquire_timeout_secs,
                })
                .await?;
                return Ok(sea_orm::SqlxSqliteConnector::from_sqlx_sqlite_pool(pool));
            }
            let mut connect = ConnectOptions::new(url.clone());
            if let Some(n) = o.max_connections {
                connect.max_connections(n);
            }
            if let Some(n) = o.min_connections {
                connect.min_connections(n);
            }
            if let Some(secs) = o.acquire_timeout_secs {
                connect.acquire_timeout(Duration::from_secs(secs));
            }
            connect.connect_lazy(true);
            Database::connect(connect).await.map_err(|error| refusal(&error, &url))
        })
    }

    /// A round trip. On SQLite the round trip is `SELECT sqlite_version()`, and a version older than
    /// 3.35 is refused as `Permanent`: SeaORM's SQLite statements use `RETURNING`, which the older
    /// engine does not parse. The connector SeaORM builds itself makes this check while connecting;
    /// the wrapped pool skips it, so an old SQLite is refused at the first ping, which is the start
    /// under `.ready` and the first health check under `.lazy()`.
    fn ping(c: &DatabaseConnection) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move {
            // Panics only on `Disconnected`, which `create` never returns.
            let backend = c.get_database_backend();
            #[cfg(feature = "sqlite")]
            if backend == DbBackend::Sqlite {
                return ping_sqlite(&c).await;
            }
            c.ping().await.map_err(|error| classify(backend, error))
        })
    }

    fn close(c: DatabaseConnection) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            if let Err(error) = c.close().await {
                tracing::debug!(%error, "closing the SeaORM pool failed");
            }
        })
    }
}

impl LazyStore for SeaOrm {}

/// SeaORM's SQLite connector sets 1 when the options name no maximum; the wrapped pool keeps it.
#[cfg(feature = "sqlite")]
const SQLITE_DEFAULT_MAX_CONNECTIONS: u32 = 1;

/// The first SQLite version whose `RETURNING` clause SeaORM's statements rely on.
#[cfg(any(feature = "sqlite", test))]
const MIN_SQLITE: (u32, u32) = (3, 35);

/// `SELECT sqlite_version()` through the connection's own pool.
#[cfg(feature = "sqlite")]
async fn ping_sqlite(c: &DatabaseConnection) -> Result<(), BoxError> {
    use sea_orm::{ConnectionTrait, Statement};

    let row = c
        .query_one_raw(Statement::from_string(DbBackend::Sqlite, "SELECT sqlite_version()"))
        .await
        .map_err(|error| classify(DbBackend::Sqlite, error))?;
    let version: String = row
        .ok_or_else(|| BoxError::from("SQLite answered no row to `SELECT sqlite_version()`"))?
        .try_get_by_index(0)
        .map_err(|error| classify(DbBackend::Sqlite, error))?;
    check_sqlite_version(&version).map_err(|refused| Permanent::new(refused).into())
}

/// A SQLite older than [`MIN_SQLITE`], or a version string this check cannot read.
#[cfg(any(feature = "sqlite", test))]
#[derive(Debug)]
struct SqliteTooOld(String);

#[cfg(any(feature = "sqlite", test))]
impl fmt::Display for SqliteTooOld {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(any(feature = "sqlite", test))]
impl Error for SqliteTooOld {}

/// Accepts `3.35` and later. The refusal names the `RETURNING` requirement and the version found.
#[cfg(any(feature = "sqlite", test))]
fn check_sqlite_version(version: &str) -> Result<(), SqliteTooOld> {
    let mut parts = version.trim().split('.').map(|part| part.parse::<u32>().ok());
    let (Some(Some(major)), Some(Some(minor))) = (parts.next(), parts.next()) else {
        return Err(SqliteTooOld(format!("could not read the SQLite version `{version}`; SeaORM needs 3.35 or later for `RETURNING`")));
    };
    if (major, minor) >= MIN_SQLITE {
        Ok(())
    } else {
        Err(SqliteTooOld(format!(
            "SQLite {version} does not support `RETURNING`, which SeaORM's statements need: 3.35 or later is required"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::check_sqlite_version;

    #[test]
    fn sqlite_before_3_35_is_refused_naming_returning_and_the_version() {
        for old in ["3.34.1", "3.34.999", "3.8.2", "2.8.17"] {
            let refused = check_sqlite_version(old).expect_err("an old SQLite passed").to_string();
            assert!(refused.contains("RETURNING"), "{refused}");
            assert!(refused.contains(old), "{refused}");
        }
    }

    #[test]
    fn sqlite_3_35_and_later_is_accepted() {
        for ok in ["3.35.0", "3.35.5", "3.46.1", "4.0.0"] {
            assert!(check_sqlite_version(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn an_unreadable_version_is_refused() {
        for bad in ["", "x", "3", "3.x"] {
            assert!(check_sqlite_version(bad).is_err(), "{bad:?}");
        }
    }
}

/// `Database::connect` failing on a lazy connection is a URL it cannot route or parse, which
/// `validate` should have refused; retrying cannot help. Its message carries the whole URL, so the
/// text is scrubbed here as well as by the registered secrets.
#[derive(Debug)]
struct ConnectRefused(String);

impl fmt::Display for ConnectRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Error for ConnectRefused {}

fn refusal(error: &DbErr, url: &str) -> BoxError {
    Permanent::new(ConnectRefused(scrub(&error.to_string(), url))).into()
}

fn classify(backend: DbBackend, error: DbErr) -> BoxError {
    if is_permanent_error(backend, &error) { Permanent::new(error).into() } else { error.into() }
}

/// `Error::Configuration`, and a database error whose code `ulo-db-sqlx` treats as permanent for
/// the engine: SQLSTATE class `28` or `3D000` on Postgres, `28000` or `42000` on MySQL, SQLite's
/// primary code 14 (`CANTOPEN`, extended codes included). `ConnectionAcquire` (a pool timeout or a
/// closed pool), `Io`, `Tls` and the rest stay bare.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn is_permanent_error(backend: DbBackend, error: &DbErr) -> bool {
    use sea_orm::{RuntimeErr, sqlx};

    let (DbErr::Conn(RuntimeErr::SqlxError(inner))
    | DbErr::Exec(RuntimeErr::SqlxError(inner))
    | DbErr::Query(RuntimeErr::SqlxError(inner))) = error
    else {
        return false;
    };
    match &**inner {
        sqlx::Error::Configuration(_) => true,
        sqlx::Error::Database(database) => code_is_permanent(backend, database.code().as_deref()),
        _ => false,
    }
}

#[cfg(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")))]
fn is_permanent_error(backend: DbBackend, error: &DbErr) -> bool {
    let _ = (backend, error);
    false
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn code_is_permanent(backend: DbBackend, code: Option<&str>) -> bool {
    let Some(code) = code else {
        return false;
    };
    match backend {
        DbBackend::Postgres => code.starts_with("28") || code == "3D000",
        DbBackend::MySql => matches!(code, "28000" | "42000"),
        DbBackend::Sqlite => code.parse::<i32>().is_ok_and(|n| n & 0xff == 14),
        _ => false,
    }
}
