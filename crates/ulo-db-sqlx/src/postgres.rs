//! Postgres as a `Store`. Advisory locks are scoped to the current database, so one key serves every
//! database.

use std::time::Duration;

use sqlx::pool::PoolConnection;
use ulo::{BoxError, BoxFuture, Permanent, Secret};
use ulo_config::Problems;
use ulo_db::{MIGRATION_LOCK_KEY, MigrationLock, Store};

use crate::classify::permanent_or_not_over;
use crate::options::{
    Engine, PgOptions, password_secrets, pool_options, require_spare_connection, unreadable_url, validate_options,
};

/// `lock_not_available`, what `lock_timeout` raises when a lock wait passes it.
const LOCK_NOT_AVAILABLE: &str = "55P03";

/// The longest `lock_timeout` Postgres takes, in milliseconds: an `int4`.
const MAX_LOCK_TIMEOUT_MS: u128 = i32::MAX as u128;

/// The Postgres store; the client is `sqlx::PgPool`.
pub struct Postgres;

impl Store for Postgres {
    const NAME: &'static str = "postgres";
    type Options = PgOptions;
    type Client = sqlx::PgPool;
    fn create_connects(_: &PgOptions) -> bool {
        false
    }

    fn validate(o: &PgOptions) -> Result<(), Problems> {
        validate_options(Engine::Postgres, &o.url, o.max_connections, o.min_connections, o.acquire_timeout_secs)
    }

    /// The URL's password component.
    fn secrets(o: &PgOptions) -> Vec<Secret<String>> {
        password_secrets(o.url.expose())
    }

    /// `connect_lazy_with`: the pool spawns its reaper at creation and connects on first use.
    fn create(o: &PgOptions) -> BoxFuture<'static, Result<sqlx::PgPool, BoxError>> {
        Box::pin(create_pool(o.clone()))
    }

    fn ping(c: &sqlx::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move {
            let tcp = pg_reaches_by_tcp(&c);
            sqlx::query("select 1").execute(&c).await.map(drop).map_err(|e| permanent_or_not_over(Engine::Postgres, e, tcp))
        })
    }

    fn close(c: sqlx::PgPool) -> BoxFuture<'static, ()> {
        Box::pin(async move { c.close().await })
    }
}

async fn create_pool(o: PgOptions) -> Result<sqlx::PgPool, BoxError> {
    let connect = o
        .url
        .expose()
        .parse::<sqlx::postgres::PgConnectOptions>()
        .map_err(|e| unreadable_url(Engine::Postgres, o.url.expose(), e))?;
    // SQLx reads the root certificate only in the modes that verify the server.
    #[cfg(feature = "tls")]
    if matches!(connect.get_ssl_mode(), sqlx::postgres::PgSslMode::VerifyCa | sqlx::postgres::PgSslMode::VerifyFull) {
        crate::options::check_root_cert(Engine::Postgres, o.url.expose())?;
    }
    Ok(pool_options::<sqlx::Postgres>(o.max_connections, o.min_connections, o.acquire_timeout_secs)
        .connect_lazy_with(connect))
}

/// Whether `pool` connects by host and port, not through a Unix socket: the reach
/// [`is_permanent_error_over_tcp`](crate::is_permanent_error_over_tcp) is for. SQLx takes a socket
/// when the options name one (a `host=/dir` parameter, a percent-encoded `/` host) and also when the
/// host itself starts with `/`, which is what `PGHOST`, or SQLx's probe of the default socket
/// directories, leaves for a URL that names no host (`postgres:///app`).
pub fn pg_reaches_by_tcp(pool: &sqlx::PgPool) -> bool {
    let options = pool.connect_options();
    options.get_socket().is_none() && !options.get_host().starts_with('/')
}

/// Every error here is classified as `ping`'s are: a refused permission or a pool too small is
/// `Permanent`, a lost connection is not.
async fn take_lock(client: sqlx::PgPool, bound: Duration) -> Result<PgLock, BoxError> {
    let tcp = pg_reaches_by_tcp(&client);
    let classify = |e: sqlx::Error| permanent_or_not_over(Engine::Postgres, e, tcp);
    require_spare_connection(Engine::Postgres, client.options().get_max_connections())
        .map_err(|e| BoxError::from(Permanent::new(e)))?;
    let mut conn = client.acquire().await.map_err(classify)?;
    conn.close_on_drop();

    // `lock_timeout` of 0 disables the timeout, so the floor is 1 ms.
    let millis = bound.as_millis().clamp(1, MAX_LOCK_TIMEOUT_MS);
    sqlx::query("select set_config('lock_timeout', $1, false)")
        .bind(format!("{millis}ms"))
        .execute(&mut *conn)
        .await
        .map_err(classify)?;

    let locked = sqlx::query("select pg_advisory_lock($1)").bind(MIGRATION_LOCK_KEY).execute(&mut *conn).await;
    match locked {
        Ok(_) => Ok(PgLock { conn }),
        Err(e) => {
            let timed_out =
                e.as_database_error().and_then(|d| d.code()).is_some_and(|code| &*code == LOCK_NOT_AVAILABLE);
            if timed_out { Err(ulo_db::LockTimedOut::new(bound).into()) } else { Err(classify(e)) }
        }
    }
}

/// The held `pg_advisory_lock`, released when the connection dies.
pub struct PgLock {
    conn: PoolConnection<sqlx::Postgres>,
}

impl MigrationLock for Postgres {
    type Guard = PgLock;

    /// `pg_advisory_lock` blocks on the server with no bound of its own, and `ulo-db` drops this
    /// future when its bound passes, which would leave the statement waiting on a connection that
    /// returns to the pool. `lock_timeout` puts the bound on the server, and the connection is
    /// marked to close on drop from the first moment, so no path hands a lock-holding session back.
    fn lock(client: &sqlx::PgPool, bound: Duration) -> BoxFuture<'static, Result<PgLock, BoxError>> {
        Box::pin(take_lock(client.clone(), bound))
    }

    /// Releases the lock and then closes the session instead of pooling it, so a failed release
    /// cannot leave the lock on a pooled connection.
    fn unlock(guard: PgLock) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let mut conn = guard.conn;
            if let Err(e) =
                sqlx::query("select pg_advisory_unlock($1)").bind(MIGRATION_LOCK_KEY).execute(&mut *conn).await
            {
                tracing::warn!(error = %e, "releasing the postgres migration lock failed; closing its connection");
            }
            if let Err(e) = conn.close().await {
                tracing::debug!(error = %e, "closing the postgres migration lock connection failed");
            }
        })
    }
}
