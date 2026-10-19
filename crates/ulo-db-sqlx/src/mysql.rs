//! MySQL as a `Store`. MySQL's locks are server-wide, so the lock name carries the database's name.

use std::time::Duration;

use sqlx::pool::PoolConnection;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, MigrationLock, Store};

use crate::classify::permanent_or_not;
use crate::options::{
    Engine, MySqlOptions, password_secrets, pool_options, require_spare_connection, unreadable_url, validate_options,
};

/// What `GET_LOCK` accepts of a lock name, in characters.
const MAX_LOCK_NAME: usize = 64;

const LOCK_PREFIX: &str = "ulo-db.migrate.";

/// The MySQL store; the client is `sqlx::MySqlPool`.
pub struct MySql;

impl Store for MySql {
    const NAME: &'static str = "mysql";
    type Options = MySqlOptions;
    type Client = sqlx::MySqlPool;
    fn create_connects(_: &MySqlOptions) -> bool {
        false
    }

    fn validate(o: &MySqlOptions) -> Result<(), Problems> {
        validate_options(Engine::MySql, &o.url, o.max_connections, o.min_connections, o.acquire_timeout_secs)
    }

    /// The URL's password component.
    fn secrets(o: &MySqlOptions) -> Vec<Secret<String>> {
        password_secrets(o.url.expose())
    }

    fn create(o: &MySqlOptions) -> BoxFuture<'static, Result<sqlx::MySqlPool, BoxError>> {
        Box::pin(create_pool(o.clone()))
    }

    fn ping(c: &sqlx::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move {
            sqlx::query("select 1").execute(&c).await.map(drop).map_err(|e| permanent_or_not(Engine::MySql, e))
        })
    }

    fn close(c: sqlx::MySqlPool) -> BoxFuture<'static, ()> {
        Box::pin(async move { c.close().await })
    }
}

impl LazyStore for MySql {}

async fn create_pool(o: MySqlOptions) -> Result<sqlx::MySqlPool, BoxError> {
    let connect = o
        .url
        .expose()
        .parse::<sqlx::mysql::MySqlConnectOptions>()
        .map_err(|e| unreadable_url(Engine::MySql, o.url.expose(), e))?;
    Ok(pool_options::<sqlx::MySql>(o.max_connections, o.min_connections, o.acquire_timeout_secs)
        .connect_lazy_with(connect))
}

/// The held `GET_LOCK` on `'ulo-db.migrate.'` followed by the database's name, a hash of the name
/// past 64 characters, MySQL's limit.
pub struct MySqlLock {
    conn: PoolConnection<sqlx::MySql>,
    name: String,
}

impl MigrationLock for MySql {
    type Guard = MySqlLock;

    /// `GET_LOCK` takes its timeout in whole seconds, rounded up here. The connection is marked to
    /// close on drop from the first moment, so a future `ulo-db` drops at its bound never hands a
    /// lock-holding session back to the pool.
    fn lock(client: &sqlx::MySqlPool, bound: Duration) -> BoxFuture<'static, Result<MySqlLock, BoxError>> {
        Box::pin(take_lock(client.clone(), bound))
    }

    /// Releases the lock and then closes the session instead of pooling it.
    fn unlock(guard: MySqlLock) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            let MySqlLock { mut conn, name } = guard;
            if let Err(e) = sqlx::query("select release_lock(?)").bind(name).execute(&mut *conn).await {
                tracing::warn!(error = %e, "releasing the mysql migration lock failed; closing its connection");
            }
            if let Err(e) = conn.close().await {
                tracing::debug!(error = %e, "closing the mysql migration lock connection failed");
            }
        })
    }
}

async fn take_lock(client: sqlx::MySqlPool, bound: Duration) -> Result<MySqlLock, BoxError> {
    require_spare_connection(Engine::MySql, client.options().get_max_connections())?;
    let mut conn = client.acquire().await?;
    conn.close_on_drop();

    let database = sqlx::query_scalar::<sqlx::MySql, Option<String>>("select database()")
        .fetch_one(&mut *conn)
        .await?
        .ok_or("the connection selected no database, so the migration lock has no name to be scoped to")?;
    let name = lock_name(&database);

    let seconds = i64::try_from(bound.as_secs() + u64::from(bound.subsec_nanos() > 0)).unwrap_or(i64::MAX);
    let got = sqlx::query_scalar::<sqlx::MySql, Option<i64>>("select get_lock(?, ?)")
        .bind(name.clone())
        .bind(seconds)
        .fetch_one(&mut *conn)
        .await?;
    match got {
        Some(1) => Ok(MySqlLock { conn, name }),
        Some(0) => Err(ulo_db::LockTimedOut::new(bound).into()),
        _ => Err("GET_LOCK failed without a result".into()),
    }
}

/// `'ulo-db.migrate.'` and the database's name, or a hash of the name when the whole would pass
/// MySQL's 64 characters. FNV-1a is stable across builds, which `DefaultHasher` is not, and every
/// replica must compute the same name. A collision only makes two databases' migrations take
/// turns.
fn lock_name(database: &str) -> String {
    let direct = format!("{LOCK_PREFIX}{database}");
    if direct.chars().count() <= MAX_LOCK_NAME {
        return direct;
    }
    format!("{LOCK_PREFIX}#{:016x}", fnv1a(database.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3))
}
