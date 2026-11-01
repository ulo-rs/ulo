//! The stores and the blocking-work value they share.

#[cfg(feature = "sqlite")]
use std::error::Error;
#[cfg(feature = "sqlite")]
use std::future::Future;
#[cfg(feature = "sqlite")]
use std::pin::Pin;
#[cfg(feature = "sqlite")]
use std::task::{Context, Poll};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::time::Duration;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use deadpool::managed::Manager;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_async::pooled_connection::AsyncDieselConnectionManager;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_async::pooled_connection::deadpool::Pool;
#[cfg(feature = "sqlite")]
use diesel_async::sync_connection_wrapper::SpawnBlocking;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use tokio::runtime::Handle;
#[cfg(feature = "sqlite")]
use tokio::runtime::RuntimeFlavor;
#[cfg(feature = "sqlite")]
use tokio::task::JoinHandle;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::{BoxError, BoxFuture, Permanent};

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use crate::options::DieselOptions;

/// A `SpawnBlocking` over the given runtime's handle: `spawn_blocking` calls
/// `handle.spawn_blocking(..)`.
///
/// `create` reads the handle once with `Handle::try_current()` inside its own future, which the
/// factory runs on the given runtime through `spawn_with` after the name check, so it is that
/// runtime's handle and finds nothing ambiently. Each SQLite connection is established through
/// `Given::spawn_blocking` in `custom_setup` and wrapped with `SyncConnectionWrapper::with_runtime`
/// and a `Given` over a clone of that handle, so neither `SyncConnectionWrapper::new` nor
/// `establish`, the two callers of `get_runtime`, is reached. The migrations and the SQLite sidecar
/// lock run through a `Given` read the same way inside the task `ulo-db` runs them in on the given
/// runtime.
///
/// Public only because it names the SQLite pool's type; it has no constructor outside the crate.
#[doc(hidden)]
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
pub struct Given {
    pub(crate) handle: Handle,
}

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
impl Given {
    /// The handle of the runtime the calling task runs on. `ulo-db` polls `create`, the ping, the
    /// migration lock and the migrations inside tasks it spawns on the given runtime after the name
    /// check, so this is the given runtime. Outside any runtime it is a permanent error, where
    /// diesel-async's default would build a runtime of its own.
    pub(crate) fn current() -> Result<Given, BoxError> {
        match Handle::try_current() {
            Ok(handle) => Ok(Given { handle }),
            Err(_) => Err(Permanent::new("the diesel store ran outside the runtime the app was given").into()),
        }
    }

    /// Runs `task` on the given runtime's blocking pool. Dropping the future detaches the task,
    /// which runs to its end: a blocking-pool thread cannot be stopped, and waiting for it would
    /// hold up whichever thread dropped the future.
    pub(crate) fn run<R: Send + 'static>(
        &self,
        task: impl FnOnce() -> R + Send + 'static,
    ) -> BoxFuture<'static, Result<R, BoxError>> {
        let join = self.handle.spawn_blocking(task);
        Box::pin(async move { join.await.map_err(BoxError::from) })
    }
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
        Box::pin(Joined { join: self.handle.spawn_blocking(task), handle: self.handle.clone() })
    }

    /// Required by the trait and reached only by `SyncConnectionWrapper::new` and `establish`,
    /// which this crate calls neither of. A `Given` is the handle of the runtime the app was given,
    /// and a function with no argument has no such runtime to read, so it panics rather than look
    /// one up or build one. A connection of the SQLite pool's type is made through the pool.
    fn get_runtime() -> Self {
        panic!(
            "ulo_db_diesel::Given has no runtime of its own; SQLite connections of this type are made by the store's pool"
        )
    }
}

/// A blocking-pool task that, dropped before it ends, waits for it on a multi-thread runtime, as
/// diesel-async's own tokio `SpawnBlocking` does. The task holds a clone of the connection's
/// `Arc<Mutex<_>>`, and a connection back in the pool while that clone lives makes the pool's
/// recycle check, which needs the connection unshared, panic. `block_in_place` hands the worker's
/// other tasks to another thread for the wait. A current-thread runtime has no other thread, and
/// `block_in_place` panics there, so on one the task is left to end by itself and that window
/// stays open.
#[cfg(feature = "sqlite")]
struct Joined<R> {
    join: JoinHandle<R>,
    handle: Handle,
}

#[cfg(feature = "sqlite")]
impl<R> Future for Joined<R> {
    type Output = Result<R, BoxError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.join).poll(cx).map_err(BoxError::from)
    }
}

#[cfg(feature = "sqlite")]
impl<R> Drop for Joined<R> {
    fn drop(&mut self) {
        if self.join.is_finished() {
            return;
        }
        match Handle::try_current().map(|current| current.runtime_flavor()) {
            Err(_) => {
                let _ = self.handle.block_on(&mut self.join);
            }
            Ok(RuntimeFlavor::MultiThread) => tokio::task::block_in_place(|| {
                let _ = self.handle.block_on(&mut self.join);
            }),
            Ok(_) => {}
        }
    }
}

/// deadpool's builder over `manager` with the options' size and wait. A wait bound needs deadpool's
/// tokio timer; without one the pool takes no runtime.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
fn build_pool<C>(manager: AsyncDieselConnectionManager<C>, o: &DieselOptions) -> Result<Pool<C>, BoxError>
where
    AsyncDieselConnectionManager<C>: Manager,
{
    let mut builder = Pool::<C>::builder(manager);
    if let Some(max) = o.max_size {
        builder = builder.max_size(max);
    }
    if let Some(secs) = o.wait_timeout_secs {
        builder = builder.wait_timeout(Some(Duration::from_secs(secs))).runtime(deadpool::Runtime::Tokio1);
    }
    builder.build().map_err(|e| Permanent::new(e).into())
}

#[cfg(feature = "postgres")]
mod postgres {
    use diesel::ConnectionError;
    use diesel::ConnectionResult;
    use diesel_async::pooled_connection::deadpool::Pool;
    use diesel_async::pooled_connection::{AsyncDieselConnectionManager, ManagerConfig};
    use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
    use tokio::runtime::Handle;
    use ulo::{BoxError, BoxFuture, Secret};
    use ulo_config::Problems;
    use ulo_db::{MigrationLock, Store};

    use super::{Given, build_pool};
    use crate::classify::permanent_or_not;
    use crate::lock::{self, DieselLock};
    use crate::options::{self, DieselOptions};

    pub type PgPool = Pool<AsyncPgConnection>;

    /// The Postgres store; the client is a deadpool pool of `AsyncPgConnection`.
    pub struct Postgres;

    impl Store for Postgres {
        const NAME: &'static str = "postgres";
        type Options = DieselOptions;
        type Client = PgPool;
        fn create_connects(_: &DieselOptions) -> bool {
            false
        }

        fn validate(o: &DieselOptions) -> Result<(), Problems> {
            options::validate_postgres(o)
        }

        /// The URL's password component.
        fn secrets(o: &DieselOptions) -> Vec<Secret<String>> {
            options::password_secrets(o.url.expose())
        }

        /// Connects nothing: deadpool's `build` makes no connection. The given runtime's handle is
        /// read here, once, for the pool's connection setup.
        fn create(o: &DieselOptions) -> BoxFuture<'static, Result<PgPool, BoxError>> {
            let o = o.clone();
            Box::pin(async move {
                let handle = Given::current()?.handle;
                let mut config = ManagerConfig::<AsyncPgConnection>::default();
                config.custom_setup = Box::new(move |url| establish(&handle, url));
                let manager = AsyncDieselConnectionManager::new_with_config(o.url.expose().clone(), config);
                build_pool(manager, &o)
            })
        }

        fn ping(c: &PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
            let pool = c.clone();
            Box::pin(async move {
                let mut conn = pool.get().await.map_err(|e| permanent_or_not(e.into()))?;
                diesel::sql_query("SELECT 1")
                    .execute(&mut *conn)
                    .await
                    .map(drop)
                    .map_err(|e| permanent_or_not(e.into()))
            })
        }

        /// deadpool's `close` returns at once; a connection still checked out closes when its
        /// holder drops it.
        fn close(c: PgPool) -> BoxFuture<'static, ()> {
            Box::pin(async move { c.close() })
        }
    }

    /// The pool's `custom_setup`: `AsyncPgConnection::establish` driven to completion on the given
    /// runtime's blocking pool. Authentication against a SCRAM server derives a key with 4096
    /// rounds of HMAC-SHA-256 inside the connect future, which in an unoptimised build takes tens
    /// of milliseconds; polled on the runtime's own thread it stalls every other task there. The
    /// connection's driver task is spawned from inside `block_on`, so it lands on the given runtime.
    fn establish<'a>(handle: &Handle, url: &'a str) -> BoxFuture<'a, ConnectionResult<AsyncPgConnection>> {
        let handle = handle.clone();
        let url = url.to_owned();
        Box::pin(async move {
            let given = Given { handle: handle.clone() };
            match given.run(move || handle.block_on(AsyncPgConnection::establish(&url))).await {
                Ok(connected) => connected,
                Err(e) => Err(ConnectionError::BadConnection(e.to_string())),
            }
        })
    }

    impl MigrationLock for Postgres {
        type Guard = DieselLock;

        fn lock(client: &PgPool, bound: std::time::Duration) -> BoxFuture<'static, Result<DieselLock, BoxError>> {
            let client = client.clone();
            Box::pin(async move { lock::postgres::lock(client, bound).await.map_err(crate::classify::lock_error) })
        }

        fn unlock(guard: DieselLock) -> BoxFuture<'static, ()> {
            Box::pin(lock::postgres::unlock(guard))
        }
    }
}

#[cfg(feature = "mysql")]
mod mysql {
    use diesel_async::pooled_connection::AsyncDieselConnectionManager;
    use diesel_async::pooled_connection::deadpool::Pool;
    use diesel_async::{AsyncMysqlConnection, RunQueryDsl};
    use ulo::{BoxError, BoxFuture, Secret};
    use ulo_config::Problems;
    use ulo_db::{MigrationLock, Store};

    use super::build_pool;
    use crate::classify::permanent_or_not;
    use crate::lock::{self, DieselLock};
    use crate::options::{self, DieselOptions};

    pub type MySqlPool = Pool<AsyncMysqlConnection>;

    /// The MySQL store; the client is a deadpool pool of `AsyncMysqlConnection`.
    pub struct MySql;

    impl Store for MySql {
        const NAME: &'static str = "mysql";
        type Options = DieselOptions;
        type Client = MySqlPool;
        fn create_connects(_: &DieselOptions) -> bool {
            false
        }

        fn validate(o: &DieselOptions) -> Result<(), Problems> {
            options::validate_mysql(o)
        }

        /// The URL's password component.
        fn secrets(o: &DieselOptions) -> Vec<Secret<String>> {
            options::password_secrets(o.url.expose())
        }

        /// Connects nothing: deadpool's `build` makes no connection.
        fn create(o: &DieselOptions) -> BoxFuture<'static, Result<MySqlPool, BoxError>> {
            let o = o.clone();
            Box::pin(async move {
                let manager = AsyncDieselConnectionManager::<AsyncMysqlConnection>::new(o.url.expose().clone());
                build_pool(manager, &o)
            })
        }

        fn ping(c: &MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
            let pool = c.clone();
            Box::pin(async move {
                let mut conn = pool.get().await.map_err(|e| permanent_or_not(e.into()))?;
                diesel::sql_query("SELECT 1")
                    .execute(&mut *conn)
                    .await
                    .map(drop)
                    .map_err(|e| permanent_or_not(e.into()))
            })
        }

        /// deadpool's `close` returns at once; a connection still checked out closes when its
        /// holder drops it.
        fn close(c: MySqlPool) -> BoxFuture<'static, ()> {
            Box::pin(async move { c.close() })
        }
    }

    impl MigrationLock for MySql {
        type Guard = DieselLock;

        fn lock(client: &MySqlPool, bound: std::time::Duration) -> BoxFuture<'static, Result<DieselLock, BoxError>> {
            let client = client.clone();
            Box::pin(async move { lock::mysql::lock(client, bound).await.map_err(crate::classify::lock_error) })
        }

        fn unlock(guard: DieselLock) -> BoxFuture<'static, ()> {
            Box::pin(lock::mysql::unlock(guard))
        }
    }
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use diesel::{Connection, ConnectionError, ConnectionResult, SqliteConnection};
    use diesel_async::RunQueryDsl;
    use diesel_async::pooled_connection::deadpool::Pool;
    use diesel_async::pooled_connection::{AsyncDieselConnectionManager, ManagerConfig};
    use diesel_async::sync_connection_wrapper::{SpawnBlocking, SyncConnectionWrapper};
    use tokio::runtime::Handle;
    use ulo::{BoxError, BoxFuture, Secret};
    use ulo_config::Problems;
    use ulo_db::{MigrationLock, Store};

    use super::{Given, build_pool};
    use crate::classify::permanent_or_not;
    use crate::lock::{self, DieselLock};
    use crate::options::{self, DieselOptions};

    type SqliteConn = SyncConnectionWrapper<SqliteConnection, Given>;

    pub type SqlitePool = Pool<SyncConnectionWrapper<SqliteConnection, Given>>;

    /// The SQLite store; each connection runs its blocking work on the given runtime's blocking pool.
    pub struct Sqlite;

    impl Store for Sqlite {
        const NAME: &'static str = "sqlite";
        type Options = DieselOptions;
        type Client = SqlitePool;
        fn create_connects(_: &DieselOptions) -> bool {
            false
        }

        fn validate(o: &DieselOptions) -> Result<(), Problems> {
            options::validate_sqlite(o)
        }

        /// None: a SQLite location holds no password.
        fn secrets(_: &DieselOptions) -> Vec<Secret<String>> {
            Vec::new()
        }

        /// Connects nothing. The given runtime's handle is read here, once, and every connection
        /// the pool makes carries a clone of it.
        fn create(o: &DieselOptions) -> BoxFuture<'static, Result<SqlitePool, BoxError>> {
            let o = o.clone();
            Box::pin(async move {
                let handle = Given::current()?.handle;
                let mut config = ManagerConfig::<SqliteConn>::default();
                config.custom_setup = Box::new(move |url| establish(&handle, url));
                let manager = AsyncDieselConnectionManager::new_with_config(o.url.expose().clone(), config);
                build_pool(manager, &o)
            })
        }

        fn ping(c: &SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
            let pool = c.clone();
            Box::pin(async move {
                let mut conn = pool.get().await.map_err(|e| permanent_or_not(e.into()))?;
                diesel::sql_query("SELECT 1")
                    .execute(&mut *conn)
                    .await
                    .map(drop)
                    .map_err(|e| permanent_or_not(e.into()))
            })
        }

        /// `close` drops the idle connections where it is called, and closing a SQLite file can
        /// checkpoint its write-ahead log, so the call runs on the blocking pool.
        fn close(c: SqlitePool) -> BoxFuture<'static, ()> {
            Box::pin(async move {
                match Given::current() {
                    Ok(given) => {
                        if let Err(e) = given.run(move || c.close()).await {
                            tracing::warn!(error = %e, "closing the sqlite pool did not finish");
                        }
                    }
                    Err(_) => c.close(),
                }
            })
        }
    }

    impl MigrationLock for Sqlite {
        type Guard = DieselLock;

        fn lock(client: &SqlitePool, bound: std::time::Duration) -> BoxFuture<'static, Result<DieselLock, BoxError>> {
            let client = client.clone();
            Box::pin(async move { lock::sqlite::lock(client, bound).await.map_err(crate::classify::lock_error) })
        }

        fn unlock(guard: DieselLock) -> BoxFuture<'static, ()> {
            Box::pin(lock::sqlite::unlock(guard))
        }
    }

    /// The pool's `custom_setup`: `SqliteConnection::establish` on the given runtime's blocking
    /// pool, the connection then wrapped with a `Given` of its own.
    fn establish<'a>(handle: &Handle, url: &'a str) -> BoxFuture<'a, ConnectionResult<SqliteConn>> {
        let mut given = Given { handle: handle.clone() };
        let url = url.to_owned();
        Box::pin(async move {
            match given.spawn_blocking(move || SqliteConnection::establish(&url)).await {
                Ok(Ok(conn)) => Ok(SyncConnectionWrapper::with_runtime(conn, given)),
                Ok(Err(e)) => Err(e),
                Err(e) => Err(ConnectionError::BadConnection(e.to_string())),
            }
        })
    }
}

#[cfg(feature = "mysql")]
pub use mysql::{MySql, MySqlPool};
#[cfg(feature = "postgres")]
pub use postgres::{PgPool, Postgres};
#[cfg(feature = "sqlite")]
pub use sqlite::{Sqlite, SqlitePool};
