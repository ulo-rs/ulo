//! The stores and the blocking-work value they share.

#[cfg(feature = "sqlite")]
use std::error::Error;

#[cfg(feature = "sqlite")]
use diesel_async::sync_connection_wrapper::SpawnBlocking;
#[cfg(feature = "sqlite")]
use tokio::runtime::Handle;
#[cfg(feature = "sqlite")]
use ulo::BoxFuture;

/// A `SpawnBlocking` over the given runtime's handle: `spawn_blocking` calls
/// `handle.spawn_blocking(..)`.
///
/// `create` reads the handle once with `Handle::current()` inside its own future, which the factory
/// runs on the given runtime through `spawn_with` after the name check, so it is that runtime's
/// handle and finds nothing ambiently. Each SQLite connection is established through
/// `Given::spawn_blocking` in `custom_setup` and wrapped with
/// `SyncConnectionWrapper::with_runtime(conn, Given(handle.clone()))`, so neither
/// `SyncConnectionWrapper::new` nor `establish`, the two callers of `get_runtime`, is reached.
///
/// Public only because it names the SQLite pool's type; it has no constructor outside the crate.
#[doc(hidden)]
#[cfg(feature = "sqlite")]
pub struct Given {
    pub(crate) handle: Handle,
}

#[cfg(feature = "sqlite")]
impl SpawnBlocking for Given {
    fn spawn_blocking<'a, R>(
        &mut self,
        task: impl FnOnce() -> R + Send + 'static,
    ) -> BoxFuture<'a, Result<R, Box<dyn Error + Send + Sync + 'static>>>
    where
        R: Send + 'static,
    {
        let _ = (&self.handle, task);
        todo!()
    }

    /// Required by the trait and reached only by `SyncConnectionWrapper::new` and `establish`,
    /// which this crate calls neither of.
    fn get_runtime() -> Self {
        todo!()
    }
}

#[cfg(feature = "postgres")]
mod postgres {
    use diesel_async::AsyncPgConnection;
    use diesel_async::pooled_connection::deadpool::Pool;
    use ulo::{BoxError, BoxFuture, Secret};
    use ulo_config::Problems;
    use ulo_db::{LazyStore, MigrationLock, Store};

    use crate::lock::DieselLock;
    use crate::options::DieselOptions;

    pub type PgPool = Pool<AsyncPgConnection>;

    /// The Postgres store; the client is a deadpool pool of `AsyncPgConnection`.
    pub struct Postgres;

    impl Store for Postgres {
        const NAME: &'static str = "postgres";
        type Options = DieselOptions;
        type Client = PgPool;
        const CREATE_CONNECTS: bool = false;

        fn validate(o: &DieselOptions) -> Result<(), Problems> {
            let _ = o;
            todo!()
        }

        fn secrets(o: &DieselOptions) -> Vec<Secret<String>> {
            let _ = o;
            todo!()
        }

        fn create(o: &DieselOptions) -> BoxFuture<'static, Result<PgPool, BoxError>> {
            let _ = o;
            todo!()
        }

        fn ping(c: &PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
            let _ = c;
            todo!()
        }

        fn close(c: PgPool) -> BoxFuture<'static, ()> {
            let _ = c;
            todo!()
        }
    }

    impl LazyStore for Postgres {}

    impl MigrationLock for Postgres {
        type Guard = DieselLock;

        fn lock(client: &PgPool, bound: std::time::Duration) -> BoxFuture<'static, Result<DieselLock, BoxError>> {
            let _ = (client, bound);
            todo!()
        }

        fn unlock(guard: DieselLock) -> BoxFuture<'static, ()> {
            let _ = guard;
            todo!()
        }
    }
}

#[cfg(feature = "mysql")]
mod mysql {
    use diesel_async::AsyncMysqlConnection;
    use diesel_async::pooled_connection::deadpool::Pool;
    use ulo::{BoxError, BoxFuture, Secret};
    use ulo_config::Problems;
    use ulo_db::{LazyStore, MigrationLock, Store};

    use crate::lock::DieselLock;
    use crate::options::DieselOptions;

    pub type MySqlPool = Pool<AsyncMysqlConnection>;

    /// The MySQL store; the client is a deadpool pool of `AsyncMysqlConnection`.
    pub struct MySql;

    impl Store for MySql {
        const NAME: &'static str = "mysql";
        type Options = DieselOptions;
        type Client = MySqlPool;
        const CREATE_CONNECTS: bool = false;

        fn validate(o: &DieselOptions) -> Result<(), Problems> {
            let _ = o;
            todo!()
        }

        fn secrets(o: &DieselOptions) -> Vec<Secret<String>> {
            let _ = o;
            todo!()
        }

        fn create(o: &DieselOptions) -> BoxFuture<'static, Result<MySqlPool, BoxError>> {
            let _ = o;
            todo!()
        }

        fn ping(c: &MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
            let _ = c;
            todo!()
        }

        fn close(c: MySqlPool) -> BoxFuture<'static, ()> {
            let _ = c;
            todo!()
        }
    }

    impl LazyStore for MySql {}

    impl MigrationLock for MySql {
        type Guard = DieselLock;

        fn lock(client: &MySqlPool, bound: std::time::Duration) -> BoxFuture<'static, Result<DieselLock, BoxError>> {
            let _ = (client, bound);
            todo!()
        }

        fn unlock(guard: DieselLock) -> BoxFuture<'static, ()> {
            let _ = guard;
            todo!()
        }
    }
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use diesel::SqliteConnection;
    use diesel_async::pooled_connection::deadpool::Pool;
    use diesel_async::sync_connection_wrapper::SyncConnectionWrapper;
    use ulo::{BoxError, BoxFuture, Secret};
    use ulo_config::Problems;
    use ulo_db::{LazyStore, MigrationLock, Store};

    use super::Given;
    use crate::lock::DieselLock;
    use crate::options::DieselOptions;

    pub type SqlitePool = Pool<SyncConnectionWrapper<SqliteConnection, Given>>;

    /// The SQLite store; each connection runs its blocking work on the given runtime's blocking pool.
    pub struct Sqlite;

    impl Store for Sqlite {
        const NAME: &'static str = "sqlite";
        type Options = DieselOptions;
        type Client = SqlitePool;
        const CREATE_CONNECTS: bool = false;

        fn validate(o: &DieselOptions) -> Result<(), Problems> {
            let _ = o;
            todo!()
        }

        fn secrets(o: &DieselOptions) -> Vec<Secret<String>> {
            let _ = o;
            todo!()
        }

        fn create(o: &DieselOptions) -> BoxFuture<'static, Result<SqlitePool, BoxError>> {
            let _ = o;
            todo!()
        }

        fn ping(c: &SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
            let _ = c;
            todo!()
        }

        fn close(c: SqlitePool) -> BoxFuture<'static, ()> {
            let _ = c;
            todo!()
        }
    }

    impl LazyStore for Sqlite {}

    impl MigrationLock for Sqlite {
        type Guard = DieselLock;

        fn lock(client: &SqlitePool, bound: std::time::Duration) -> BoxFuture<'static, Result<DieselLock, BoxError>> {
            let _ = (client, bound);
            todo!()
        }

        fn unlock(guard: DieselLock) -> BoxFuture<'static, ()> {
            let _ = guard;
            todo!()
        }
    }
}

#[cfg(feature = "mysql")]
pub use mysql::{MySql, MySqlPool};
#[cfg(feature = "postgres")]
pub use postgres::{PgPool, Postgres};
#[cfg(feature = "sqlite")]
pub use sqlite::{Sqlite, SqlitePool};
