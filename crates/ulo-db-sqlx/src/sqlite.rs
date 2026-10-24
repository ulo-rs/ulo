//! SQLite as a `Store`. The migration lock is an OS lock on a sidecar file, taken off the worker.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use ulo::{BoxError, BoxFuture, Permanent, Secret};
use ulo_config::Problems;
use ulo_db::{MigrationLock, Store};

use crate::classify::permanent_or_not;
use crate::options::{Engine, SqliteOptions, pool_options, unreadable_url, validate_options};

/// How often the lock thread retries a lock another process holds.
const POLL: Duration = Duration::from_millis(50);

/// How long before the bound the lock thread makes its last attempt. The caller's own clock for
/// the bound started before the thread did and runs in whole milliseconds, so an attempt at the
/// bound itself reports after the caller has given up. The wait ends with one attempt at this
/// margin, not a whole poll short of the bound, so a lock freed in the last poll interval is
/// still taken.
const LAST_ATTEMPT_MARGIN: Duration = Duration::from_millis(10);

const SIDECAR_SUFFIX: &str = ".ulo-migrate.lock";

/// The SQLite store; the client is `sqlx::SqlitePool`.
pub struct Sqlite;

impl Store for Sqlite {
    const NAME: &'static str = "sqlite";
    type Options = SqliteOptions;
    type Client = sqlx::SqlitePool;
    fn create_connects(_: &SqliteOptions) -> bool {
        false
    }

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
    // Read before the query below and the thread's start, which the caller's clock also counts.
    let started = Instant::now();
    let file = sqlx::query_scalar::<sqlx::Sqlite, Option<String>>(
        "select file from pragma_database_list where name = 'main'",
    )
    .fetch_one(&client)
    .await
    .map_err(|e| permanent_or_not(Engine::Sqlite, e))?;
    let Some(file) = file.filter(|f| !f.is_empty()) else {
        return Ok(SqliteLock { file: None });
    };

    let mut sidecar = std::ffi::OsString::from(file);
    sidecar.push(SIDECAR_SUFFIX);
    let sidecar = std::path::PathBuf::from(sidecar);

    let file = on_lock_thread(move || acquire(&sidecar, bound, started)).await?;
    Ok(SqliteLock { file: Some(file) })
}

/// Runs `work` on a thread of its own and awaits its answer. A panic in `work` is `Permanent` and
/// logged at `error` with its message: a retry runs the same code on the same file and panics
/// again, where a start that took it for a lost answer would retry to the bound.
async fn on_lock_thread(work: impl FnOnce() -> Result<File, BoxError> + Send + 'static) -> Result<File, BoxError> {
    let (answer, wait) = futures_channel::oneshot::channel();
    std::thread::Builder::new()
        .name("ulo-db-sqlite-lock".to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).unwrap_or_else(|panic| {
                let message = panic_message(&*panic);
                tracing::error!(panic = %message, "the SQLite migration lock thread panicked");
                Err(Permanent::new("the SQLite migration lock thread panicked").into())
            });
            // A send to a dropped receiver hands the file back, and dropping it releases the lock.
            let _ = answer.send(outcome);
        })
        .map_err(|e| format!("could not start the thread that takes the SQLite migration lock: {e}"))?;

    match wait.await {
        Ok(outcome) => outcome,
        Err(_) => Err("the SQLite migration lock thread ended without an answer".into()),
    }
}

/// The text of a panic payload: a `&str` or a `String`, which is what `panic!` raises.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    match payload.downcast_ref::<&str>() {
        Some(message) => (*message).to_owned(),
        None => payload.downcast_ref::<String>().cloned().unwrap_or_else(|| "a payload that is not text".to_owned()),
    }
}

fn acquire(path: &Path, bound: Duration, started: Instant) -> Result<File, BoxError> {
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| BoxError::from(Permanent::new(format!("could not open {}: {e}", path.display()))))?;
    // `None` when the bound is too large for an `Instant`: wait without end.
    let last_attempt = started.checked_add(bound).map(|d| d.checked_sub(LAST_ATTEMPT_MARGIN).unwrap_or(started));
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                let pause = match last_attempt {
                    None => POLL,
                    // The last sleep ends at the last attempt instead of a poll past it, and that
                    // attempt is made before the wait is declared over.
                    Some(at) => match at.checked_duration_since(Instant::now()) {
                        Some(left) if !left.is_zero() => left.min(POLL),
                        _ => return Err(ulo_db::LockTimedOut::new(bound).into()),
                    },
                };
                std::thread::sleep(pause);
            }
            Err(TryLockError::Error(e)) => {
                return Err(Permanent::new(format!("could not lock {}: {e}", path.display())).into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex, PoisonError};

    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Event, Level, Metadata, Subscriber};

    use super::*;

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

    /// A lock thread that panics ends the wait as `Permanent`, once, and logs the panic's own
    /// message at `error`. The thread's other outcomes are untouched: a lock it took is returned
    /// and an error it reports passes through as it is.
    #[test]
    fn a_panicking_lock_thread_is_permanent_and_logged_at_error_with_its_message() {
        // The event is logged on the lock thread, so the subscriber is the process's: this is the
        // only test in the crate that installs one.
        let capture = Capture::default();
        tracing::subscriber::set_global_default(capture.clone()).expect("no other subscriber is installed");
        let error = run(on_lock_thread(|| panic!("the sidecar blew up"))).expect_err("a panic was taken for a lock");
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

        let file = tempfile_in_scratch();
        let held = run(on_lock_thread(move || Ok(file))).expect("a lock the thread took is returned");
        drop(held);
        let refused = run(on_lock_thread(|| Err(ulo_db::LockTimedOut::new(Duration::from_millis(1)).into()))).expect_err("an error passes");
        assert!(refused.is::<ulo_db::LockTimedOut>(), "an error the thread reports passes through as it is: {refused}");
        assert!(!refused.is::<Permanent>());
    }

    fn tempfile_in_scratch() -> File {
        let path = std::env::temp_dir().join(format!("ulo-db-sqlx-lock-thread-{}.lock", std::process::id()));
        let file = OpenOptions::new().write(true).create(true).truncate(true).open(&path).expect("a scratch file");
        let _ = std::fs::remove_file(&path);
        file
    }
}
