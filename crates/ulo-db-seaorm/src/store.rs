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
    const CREATE_CONNECTS: bool = false;

    fn validate(o: &SeaOrmOptions) -> Result<(), Problems> {
        crate::options::validate(o)
    }

    /// The URL's password component, beside the whole URL the section read registered.
    fn secrets(o: &SeaOrmOptions) -> Vec<Secret<String>> {
        password_texts(o.url.expose()).into_iter().map(Secret::new).collect()
    }

    /// `ConnectOptions::connect_lazy(true)`: connects nothing.
    fn create(o: &SeaOrmOptions) -> BoxFuture<'static, Result<DatabaseConnection, BoxError>> {
        let o = o.clone();
        Box::pin(async move {
            let url = o.url.expose().clone();
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

    fn ping(c: &DatabaseConnection) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move {
            // Panics only on `Disconnected`, which `create` never returns.
            let backend = c.get_database_backend();
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
