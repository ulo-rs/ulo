//! The migration lock's three primitives over diesel-async's connections.
//!
//! Postgres: `pg_advisory_lock(K)` on a connection held for the migration. MySQL: `GET_LOCK` on
//! `'ulo-db.migrate.'` followed by the database's name, hashed past MySQL's 64 characters. SQLite: an
//! OS lock on `<file>.ulo-migrate.lock` through `std::fs::File::lock`, taken off the worker, and none
//! for an in-memory database.
//!
//! The Postgres and MySQL connection comes from the pool's manager and never from the pool, so a
//! pool of one still has its connection free for the migrator, and the session holding the lock is
//! closed, never pooled, once the guard is released or dropped.

use std::any::Any;

/// The held lock: the dedicated connection, or the locked sidecar file. Dropping it releases the
/// lock, as `unlock` does.
pub struct DieselLock {
    // Read by the engine modules, which each need an engine feature.
    #[cfg_attr(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")), allow(dead_code))]
    pub(crate) held: Box<dyn Any + Send>,
}

#[cfg(feature = "postgres")]
pub(crate) mod postgres {
    use std::time::Duration;

    use deadpool::managed::Manager;
    use diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
    use ulo::BoxError;
    use ulo_db::MIGRATION_LOCK_KEY;

    use super::DieselLock;
    use crate::store::PgPool;

    struct Held(AsyncPgConnection);

    /// `lock_timeout` holds the wait to `bound` on the server as well, so a session `ulo-db` gives
    /// up on at its bound stops waiting rather than taking the lock after the start has failed.
    pub(crate) async fn lock(pool: PgPool, bound: Duration) -> Result<DieselLock, BoxError> {
        let mut conn = pool.manager().create().await?;
        let millis = bound.as_millis().clamp(1, i32::MAX as u128);
        conn.batch_execute(&format!("SET lock_timeout = {millis}")).await?;
        conn.batch_execute(&format!("SELECT pg_advisory_lock({MIGRATION_LOCK_KEY})")).await?;
        Ok(DieselLock { held: Box::new(Held(conn)) })
    }

    pub(crate) async fn unlock(guard: DieselLock) {
        let Ok(held) = guard.held.downcast::<Held>() else {
            return;
        };
        let Held(mut conn) = *held;
        if let Err(e) = conn.batch_execute(&format!("SELECT pg_advisory_unlock({MIGRATION_LOCK_KEY})")).await {
            tracing::warn!(error = %e, "releasing the postgres migration lock failed; closing its session releases it");
        }
    }
}

#[cfg(feature = "mysql")]
pub(crate) mod mysql {
    use std::time::Duration;

    use deadpool::managed::Manager;
    use diesel::dsl::sql;
    use diesel::sql_types::{BigInt, Nullable, Text};
    use diesel_async::{AsyncMysqlConnection, RunQueryDsl};
    use ulo::BoxError;

    use super::DieselLock;
    use crate::store::MySqlPool;

    /// What `GET_LOCK` accepts of a lock name, in characters.
    const MAX_LOCK_NAME: usize = 64;

    const LOCK_PREFIX: &str = "ulo-db.migrate.";

    struct Held {
        conn: AsyncMysqlConnection,
        name: String,
    }

    /// `GET_LOCK` takes its timeout in whole seconds, rounded up here.
    pub(crate) async fn lock(pool: MySqlPool, bound: Duration) -> Result<DieselLock, BoxError> {
        let mut conn = pool.manager().create().await?;
        let database: Option<String> = diesel::select(sql::<Nullable<Text>>("DATABASE()")).get_result(&mut conn).await?;
        let database =
            database.ok_or("the connection selected no database, so the migration lock has no name to be scoped to")?;
        let name = lock_name(&database);
        let seconds = i64::try_from(bound.as_secs().saturating_add(u64::from(bound.subsec_nanos() > 0))).unwrap_or(i64::MAX);
        let got: Option<i64> = diesel::select(
            sql::<Nullable<BigInt>>("GET_LOCK(")
                .bind::<Text, _>(name.clone())
                .sql(", ")
                .bind::<BigInt, _>(seconds)
                .sql(")"),
        )
        .get_result(&mut conn)
        .await?;
        match got {
            Some(1) => Ok(DieselLock { held: Box::new(Held { conn, name }) }),
            Some(0) => Err(ulo_db::LockTimedOut::new(bound).into()),
            _ => Err("GET_LOCK failed without a result".into()),
        }
    }

    pub(crate) async fn unlock(guard: DieselLock) {
        let Ok(held) = guard.held.downcast::<Held>() else {
            return;
        };
        let Held { mut conn, name } = *held;
        let released = diesel::select(sql::<Nullable<BigInt>>("RELEASE_LOCK(").bind::<Text, _>(name).sql(")"))
            .execute(&mut conn)
            .await;
        if let Err(e) = released {
            tracing::warn!(error = %e, "releasing the mysql migration lock failed; closing its session releases it");
        }
    }

    /// `'ulo-db.migrate.'` and the database's name, or a hash of the name when the whole would pass
    /// MySQL's 64 characters. The spelling is `ulo-db-sqlx`'s, so a SQLx store and a Diesel store
    /// migrating one database take one lock. FNV-1a is stable across builds, which `DefaultHasher`
    /// is not, and every replica must compute the same name; a collision only makes two databases'
    /// migrations take turns.
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
}

#[cfg(feature = "sqlite")]
pub(crate) mod sqlite {
    use std::fs::{File, OpenOptions, TryLockError};
    use std::time::{Duration, Instant};

    use diesel::dsl::sql;
    use diesel::sql_types::{Nullable, Text};
    use diesel_async::RunQueryDsl;
    use ulo::{BoxError, Permanent};

    use super::DieselLock;
    use crate::store::{Given, SqlitePool};

    /// How often a waiting lock is tried again. `File::lock` would wait with no bound, so the wait
    /// polls `try_lock` and its blocking-pool thread ends by itself at the bound.
    const POLL: Duration = Duration::from_millis(50);

    struct Held(Option<File>);

    /// The sidecar sits beside the file SQLite resolved for the connection's `main` database,
    /// which is absolute, so replicas reaching one file through different relative paths take one
    /// lock. An in-memory or temporary database reports no file and takes none.
    pub(crate) async fn lock(pool: SqlitePool, bound: Duration) -> Result<DieselLock, BoxError> {
        let deadline = Instant::now().checked_add(bound);
        let file: Option<String> = {
            let mut conn = pool.get().await?;
            diesel::select(sql::<Nullable<Text>>("(SELECT file FROM pragma_database_list WHERE name = 'main')"))
                .get_result(&mut *conn)
                .await?
        };
        let Some(file) = file.filter(|f| !f.is_empty()) else {
            return Ok(DieselLock { held: Box::new(Held(None)) });
        };
        let locked = Given::current()?.run(move || lock_sidecar(&file, bound, deadline)).await??;
        Ok(DieselLock { held: Box::new(Held(Some(locked))) })
    }

    pub(crate) async fn unlock(guard: DieselLock) {
        let Ok(held) = guard.held.downcast::<Held>() else {
            return;
        };
        if let Held(Some(file)) = *held
            && let Err(e) = file.unlock()
        {
            tracing::debug!(error = %e, "unlocking the sqlite migration sidecar failed; closing it releases the lock");
        }
    }

    /// Runs on a blocking-pool thread. The sidecar is never removed: a file deleted while another
    /// process waits on its lock would let a third take a lock on a new file at the same path.
    fn lock_sidecar(database: &str, bound: Duration, deadline: Option<Instant>) -> Result<File, BoxError> {
        let path = format!("{database}.ulo-migrate.lock");
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&path).map_err(|e| {
            // A sidecar that cannot be opened is a path or permission no restart mends.
            BoxError::from(Permanent::new(format!("could not open {path}: {e}")))
        })?;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) => {}
                Err(TryLockError::Error(e)) => {
                    return Err(Permanent::new(format!("could not lock {path}: {e}")).into());
                }
            }
            let pause = match deadline {
                None => POLL,
                Some(deadline) => match deadline.checked_duration_since(Instant::now()) {
                    Some(left) if !left.is_zero() => left.min(POLL),
                    _ => return Err(ulo_db::LockTimedOut::new(bound).into()),
                },
            };
            std::thread::sleep(pause);
        }
    }
}
