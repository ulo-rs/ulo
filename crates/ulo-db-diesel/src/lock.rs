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
        // A URL that names no database is the operator's to mend, as `ulo-db-sqlx` marks it.
        let database = database.ok_or_else(|| {
            ulo::Permanent::new("the connection selected no database, so the migration lock has no name to be scoped to")
        })?;
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

    /// How long before the bound the last attempt is made. The caller's own clock for the bound
    /// started before this wait did and runs in whole milliseconds, so an attempt at the bound
    /// itself reports after the caller has given up. A lock freed in the last poll interval is
    /// still taken by the attempt at this margin.
    const LAST_ATTEMPT_MARGIN: Duration = Duration::from_millis(10);

    struct Held(Option<File>);

    /// The sidecar sits beside the file SQLite resolved for the connection's `main` database,
    /// which is absolute, so replicas reaching one file through different relative paths take one
    /// lock. An in-memory or temporary database reports no file and takes none.
    pub(crate) async fn lock(pool: SqlitePool, bound: Duration) -> Result<DieselLock, BoxError> {
        let started = Instant::now();
        let deadline = started.checked_add(bound).map(|d| d.checked_sub(LAST_ATTEMPT_MARGIN).unwrap_or(started));
        let file: Option<String> = {
            let mut conn = pool.get().await?;
            diesel::select(sql::<Nullable<Text>>("(SELECT file FROM pragma_database_list WHERE name = 'main')"))
                .get_result(&mut *conn)
                .await?
        };
        let Some(file) = file.filter(|f| !f.is_empty()) else {
            return Ok(DieselLock { held: Box::new(Held(None)) });
        };
        let locked = on_blocking_thread(move || lock_sidecar(&file, bound, deadline)).await?;
        Ok(DieselLock { held: Box::new(Held(Some(locked))) })
    }

    /// Runs `work` on the given runtime's blocking pool. A panic in `work` is `Permanent` and
    /// logged at `error` with its message: a retry runs the same code on the same file and panics
    /// again, where a start that took the join error for a lost answer would retry to the bound.
    pub(super) async fn on_blocking_thread(
        work: impl FnOnce() -> Result<File, BoxError> + Send + 'static,
    ) -> Result<File, BoxError> {
        Given::current()?
            .run(move || {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
                    let message = panic_message(&*panic);
                    tracing::error!(panic = %message, "the SQLite migration lock thread panicked");
                    Err(Permanent::new("the SQLite migration lock thread panicked").into())
                })
            })
            .await?
    }

    /// The text of a panic payload: a `&str` or a `String`, which is what `panic!` raises.
    fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
        match payload.downcast_ref::<&str>() {
            Some(message) => (*message).to_owned(),
            None => payload.downcast_ref::<String>().cloned().unwrap_or_else(|| "a payload that is not text".to_owned()),
        }
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

#[cfg(all(test, feature = "sqlite"))]
mod tests {
    use std::fs::OpenOptions;
    use std::sync::{Arc, Mutex, PoisonError};
    use std::time::Duration;

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};
    use ulo::Permanent;

    use super::sqlite::on_blocking_thread;

    /// Every event logged while it is the default, with its level and fields.
    type Events = Vec<(Level, Vec<(String, String)>)>;

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Events>>);

    struct Fields(Vec<(String, String)>);

    impl Visit for Fields {
        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.0.push((field.name().to_owned(), format!("{value:?}")));
        }

        fn record_str(&mut self, field: &Field, value: &str) {
            self.0.push((field.name().to_owned(), value.to_owned()));
        }
    }

    impl Subscriber for Capture {
        fn enabled(&self, _: &Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }
        fn record(&self, _: &Id, _: &Record<'_>) {}
        fn record_follows_from(&self, _: &Id, _: &Id) {}
        fn event(&self, event: &Event<'_>) {
            let mut fields = Fields(Vec::new());
            event.record(&mut fields);
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((*event.metadata().level(), fields.0));
        }
        fn enter(&self, _: &Id) {}
        fn exit(&self, _: &Id) {}
    }

    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread().build().expect("a runtime").block_on(future)
    }

    /// A lock thread that panics ends the wait as `Permanent` and logs the panic's own message at
    /// `error`, once. A lock the thread took is returned, and an error it reports passes through
    /// as it is.
    #[test]
    fn a_panicking_lock_thread_is_permanent_and_logged_at_error_with_its_message() {
        // The event is logged on the lock thread, so the subscriber is the process's: this is the
        // only test in the crate that installs one.
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone()).expect("no other subscriber is installed");
        let error = run(on_blocking_thread(|| panic!("the sidecar blew up"))).expect_err("a panic was taken for a lock");
        assert!(error.is::<Permanent>(), "a panic in the lock thread is retryable: {error}");
        let logged: Vec<_> = capture
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, fields)| fields.iter().any(|(name, _)| name == "panic"))
            .cloned()
            .collect();
        assert_eq!(logged.len(), 1, "one event for one panic: {logged:?}");
        assert_eq!(logged[0].0, Level::ERROR);
        assert!(logged[0].1.contains(&("panic".to_owned(), "the sidecar blew up".to_owned())), "the panic message: {logged:?}");

        let path = std::env::temp_dir().join(format!("ulo-db-diesel-lock-thread-{}.lock", std::process::id()));
        let file = OpenOptions::new().write(true).create(true).truncate(true).open(&path).expect("a scratch file");
        let _ = std::fs::remove_file(&path);
        run(on_blocking_thread(move || Ok(file))).expect("a lock the thread took is returned");
        let refused = run(on_blocking_thread(|| Err(ulo_db::LockTimedOut::new(Duration::from_millis(1)).into())))
            .expect_err("an error passes");
        assert!(refused.is::<ulo_db::LockTimedOut>(), "an error the thread reports passes through as it is: {refused}");
        assert!(!refused.is::<Permanent>());
    }
}
