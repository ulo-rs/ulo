//! MySQL as a `Store`. MySQL's locks are server-wide, so the lock name carries the database's name.

use std::time::Duration;

use sqlx::pool::PoolConnection;
use ulo::{BoxError, BoxFuture, Secret};
use ulo_config::Problems;
use ulo_db::{LazyStore, MigrationLock, Store};

use crate::options::MySqlOptions;

/// The MySQL store; the client is `sqlx::MySqlPool`.
pub struct MySql;

impl Store for MySql {
    const NAME: &'static str = "mysql";
    type Options = MySqlOptions;
    type Client = sqlx::MySqlPool;
    const CREATE_CONNECTS: bool = false;

    fn validate(o: &MySqlOptions) -> Result<(), Problems> {
        let _ = o;
        todo!()
    }

    /// The URL's password component.
    fn secrets(o: &MySqlOptions) -> Vec<Secret<String>> {
        let _ = o;
        todo!()
    }

    fn create(o: &MySqlOptions) -> BoxFuture<'static, Result<sqlx::MySqlPool, BoxError>> {
        let _ = o;
        todo!()
    }

    fn ping(c: &sqlx::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = c;
        todo!()
    }

    fn close(c: sqlx::MySqlPool) -> BoxFuture<'static, ()> {
        let _ = c;
        todo!()
    }
}

impl LazyStore for MySql {}

/// The held `GET_LOCK` on `'ulo-db.migrate.'` followed by the database's name, a hash of the name
/// past 64 characters, MySQL's limit.
pub struct MySqlLock {
    conn: PoolConnection<sqlx::MySql>,
    name: String,
}

impl MigrationLock for MySql {
    type Guard = MySqlLock;

    fn lock(client: &sqlx::MySqlPool, bound: Duration) -> BoxFuture<'static, Result<MySqlLock, BoxError>> {
        let _ = (client, bound);
        todo!()
    }

    fn unlock(guard: MySqlLock) -> BoxFuture<'static, ()> {
        let _ = (guard.conn, guard.name);
        todo!()
    }
}
