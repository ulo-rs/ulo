//! SQLite as a `Store`. The migration lock is an OS lock on a sidecar file, taken off the worker.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, MigrationLock, Store};

use crate::classify::permanent_or_not;
use crate::options::{Engine, SqliteOptions, pool_options, unreadable_url, validate_options};

/// How often the lock thread retries a lock another process holds.
const POLL: Duration = Duration::from_millis(50);

const SIDECAR_SUFFIX: &str = ".ulo-migrate.lock";

/// The SQLite store; the client is `sqlx::SqlitePool`.
pub struct Sqlite;

impl Store for Sqlite {
    const NAME: &'static str = "sqlite";
    type Options = SqliteOptions;
    type Client = sqlx::SqlitePool;
    const CREATE_CONNECTS: bool = false;

    fn validate(o: &SqliteOptions) -> Result<(), Problems> {
        validate_options(Engine::Sqlite, &o.url, o.max_connections, o.min_connections, o.acquire_timeout_secs)
    }

    /// A SQLite URL holds no password, so nothing beyond the URL itself.
    fn secrets(o: &SqliteOptions) -> Vec<Secret<String>> {
        let _ = o;
        Vec::new()
    }

    fn create(o: &SqliteOptions) -> BoxFuture<'static, Result<sqlx::SqlitePool, BoxError>> {
        Box::pin(create_pool(o.clone()))
    }

    fn ping(c: &sqlx::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        let c = c.clone();
        Box::pin(async move {
            sqlx::query("select 1").execute(&c).await.map(drop).map_err(|e| permanent_or_not(Engine::Sqlite, e))
        })
    }

    fn close(c: sqlx::SqlitePool) -> BoxFuture<'static, ()> {
        Box::pin(async move { c.close().await })
    }
}

impl LazyStore for Sqlite {}

async fn create_pool(o: SqliteOptions) -> Result<sqlx::SqlitePool, BoxError> {
    let connect = o
        .url
        .expose()
        .parse::<sqlx::sqlite::SqliteConnectOptions>()
        .map_err(|e| unreadable_url(Engine::Sqlite, o.url.expose(), e))?;
    let mut pool = pool_options::<sqlx::Sqlite>(o.max_connections, o.min_connections, o.acquire_timeout_secs);
    if is_in_memory(o.url.expose()) {
        // An in-memory database lives while a connection to it is open. SQLx reaps idle connections
        // after ten minutes and retires every connection after thirty, which would drop the data.
        pool = pool.idle_timeout(None).max_lifetime(None);
    }
    Ok(pool.connect_lazy_with(connect))
}

/// Whether SQLx opens `url` as an in-memory database: the `:memory:` name or `mode=memory`. SQLx
/// keeps the flag private, and reads the URL with exactly this prefix stripping and `?` split.
fn is_in_memory(url: &str) -> bool {
    let rest = url.trim_start_matches("sqlite://").trim_start_matches("sqlite:");
    let (database, params) = rest.split_once('?').unwrap_or((rest, ""));
    database == ":memory:" || params.split('&').any(|p| p == "mode=memory")
}

/// The locked `<file>.ulo-migrate.lock`, released on drop; `None` for an in-memory database, which
/// needs none. `std::fs::File::lock` is stable from Rust 1.89.
pub struct SqliteLock {
    file: Option<File>,
}

impl MigrationLock for Sqlite {
    type Guard = SqliteLock;

    /// The file is found by asking SQLite for the main database's path, which answers an absolute
    /// path and an empty one for a database with no file, so replicas that spell one path two ways
    /// still lock the same sidecar and in-memory and temporary databases take none.
    ///
    /// Waiting on a file lock blocks the thread that waits, so the wait runs on a thread of its own
    /// and the future awaits its answer. It polls `try_lock` to the bound instead of blocking in
    /// `lock`, so a thread whose future was dropped ends at the bound and never outlives it holding
    /// the file.
    fn lock(client: &sqlx::SqlitePool, bound: Duration) -> BoxFuture<'static, Result<SqliteLock, BoxError>> {
        Box::pin(take_lock(client.clone(), bound))
    }

    fn unlock(guard: SqliteLock) -> BoxFuture<'static, ()> {
        Box::pin(async move {
            if let Some(file) = guard.file {
                // Closing the file releases the lock too; unlocking first frees it ahead of the close.
                let _ = file.unlock();
            }
        })
    }
}

async fn take_lock(client: sqlx::SqlitePool, bound: Duration) -> Result<SqliteLock, BoxError> {
    let file = sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "select file from pragma_database_list where name = 'main'",
    )
    .fetch_one(&client)
    .await?;
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return Ok(SqliteLock { file: None });
    };

    let mut sidecar = std::ffi::OsString::from(file);
    sidecar.push(SIDECAR_SUFFIX);
    let sidecar = std::path::PathBuf::from(sidecar);

    let (answer, wait) = futures_channel::oneshot::channel();
    std::thread::Builder::new()
        .name("ulo-db-sqlite-lock".to_owned())
        .spawn(move || {
            // A send to a dropped receiver hands the file back, and dropping it releases the lock.
            let _ = answer.send(acquire(&sidecar, bound));
        })
        .map_err(|e| format!("could not start the thread that takes the SQLite migration lock: {e}"))?;

    match wait.await {
        Ok(Ok(file)) => Ok(SqliteLock { file: Some(file) }),
        Ok(Err(message)) => Err(message.into()),
        Err(_) => Err("the SQLite migration lock thread ended without an answer".into()),
    }
}

fn acquire(path: &Path, bound: Duration) -> Result<File, String> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| format!("could not open {}: {e}", path.display()))?;
    // `None` when the bound is too large for an `Instant`: wait without end.
    let deadline = Instant::now().checked_add(bound);
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return Err(format!("the migration lock on {} was not obtained within {bound:?}", path.display()));
                }
                std::thread::sleep(POLL);
            }
            Err(TryLockError::Error(e)) => return Err(format!("could not lock {}: {e}", path.display())),
        }
    }
}
