//! Postgres as a `Store`. Advisory locks are scoped to the current database, so one key serves every
//! database.

use std::time::Duration;

use sqlx::pool::PoolConnection;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, MigrationLock, Store};

use crate::options::PgOptions;

/// The Postgres store; the client is `sqlx::PgPool`.
pub struct Postgres;

impl Store for Postgres {
    const NAME: &'static str = "postgres";
    type Options = PgOptions;
    type Client = sqlx::PgPool;
    const CREATE_CONNECTS: bool = false;

    fn validate(o: &PgOptions) -> Result<(), Problems> {
        let _ = o;
        todo!()
    }

    /// The URL's password component.
    fn secrets(o: &PgOptions) -> Vec<Secret<String>> {
        let _ = o;
        todo!()
    }

    /// `connect_lazy_with`: the pool spawns its reaper at creation and connects on first use.
    fn create(o: &PgOptions) -> BoxFuture<'static, Result<sqlx::PgPool, BoxError>> {
        let _ = o;
        todo!()
    }

    fn ping(c: &sqlx::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = c;
        todo!()
    }

    fn close(c: sqlx::PgPool) -> BoxFuture<'static, ()> {
        let _ = c;
        todo!()
    }
}

impl LazyStore for Postgres {}

/// The held `pg_advisory_lock`, released when the connection dies.
pub struct PgLock {
    conn: PoolConnection<sqlx::Postgres>,
}

impl MigrationLock for Postgres {
    type Guard = PgLock;

    fn lock(client: &sqlx::PgPool, bound: Duration) -> BoxFuture<'static, Result<PgLock, BoxError>> {
        let _ = (client, bound);
        todo!()
    }

    fn unlock(guard: PgLock) -> BoxFuture<'static, ()> {
        let _ = guard.conn;
        todo!()
    }
}
