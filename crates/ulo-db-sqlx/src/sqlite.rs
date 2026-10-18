//! SQLite as a `Store`. The migration lock is an OS lock on a sidecar file, taken off the worker.

use std::fs::File;
use std::time::Duration;

use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, MigrationLock, Store};

use crate::options::SqliteOptions;

/// The SQLite store; the client is `sqlx::SqlitePool`.
pub struct Sqlite;

impl Store for Sqlite {
    const NAME: &'static str = "sqlite";
    type Options = SqliteOptions;
    type Client = sqlx::SqlitePool;
    const CREATE_CONNECTS: bool = false;

    fn validate(o: &SqliteOptions) -> Result<(), Problems> {
        let _ = o;
        todo!()
    }

    /// A SQLite URL holds no password, so nothing beyond the URL itself.
    fn secrets(o: &SqliteOptions) -> Vec<Secret<String>> {
        let _ = o;
        todo!()
    }

    fn create(o: &SqliteOptions) -> BoxFuture<'static, Result<sqlx::SqlitePool, BoxError>> {
        let _ = o;
        todo!()
    }

    fn ping(c: &sqlx::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = c;
        todo!()
    }

    fn close(c: sqlx::SqlitePool) -> BoxFuture<'static, ()> {
        let _ = c;
        todo!()
    }
}

impl LazyStore for Sqlite {}

/// The locked `<file>.ulo-migrate.lock`, released on drop; `None` for an in-memory database, which
/// needs none. `std::fs::File::lock` is stable from Rust 1.89.
pub struct SqliteLock {
    file: Option<File>,
}

impl MigrationLock for Sqlite {
    type Guard = SqliteLock;

    fn lock(client: &sqlx::SqlitePool, bound: Duration) -> BoxFuture<'static, Result<SqliteLock, BoxError>> {
        let _ = (client, bound);
        todo!()
    }

    fn unlock(guard: SqliteLock) -> BoxFuture<'static, ()> {
        let _ = guard.file;
        todo!()
    }
}
