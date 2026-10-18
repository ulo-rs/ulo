//! `diesel_migrations` through `AsyncConnectionWrapper`, which blocks on futures and panics on a
//! tokio worker. The closure runs through `Given` on the given runtime's blocking pool, where
//! `AsyncConnectionWrapper`'s tokio `BlockOn` reads `Handle::try_current()` and finds that
//! runtime. `AsyncMigrationHarness` uses `block_in_place`, which panics on a current-thread runtime,
//! and is not used. `diesel_migrations` takes no lock, so `ulo-db`'s lock wraps the run.
//!
//! The migrations run on a connection from the pool, so an in-memory SQLite database with a pool of
//! one is migrated on the connection the application then uses. When `ulo-db`'s bound passes first,
//! the closure is left to finish on its blocking-pool thread, since a thread cannot be cancelled,
//! and the start fails with `MigrationFailed`.

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::sync::Arc;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel::backend::Backend;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel::migration::{Migration, MigrationSource};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_async::async_connection_wrapper::AsyncConnectionWrapper;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_async::pooled_connection::deadpool::Object;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel_migrations::MigrationHarness;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::{BoxError, BoxFuture};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo_db::Migrations;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use crate::store::Given;

/// A `diesel_migrations` source, `embed_migrations!()`'s value or a directory's, for
/// [`StoreModule::migrate`](ulo_db::StoreModule::migrate).
pub struct DieselMigrations<M> {
    #[cfg_attr(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")), allow(dead_code))]
    pub(crate) source: M,
}

impl<M> DieselMigrations<M> {
    pub fn new(source: M) -> Self {
        DieselMigrations { source }
    }
}

/// The source behind the module's shared handle, since `run_pending_migrations` takes its source
/// by value and the module keeps the migrations for every run.
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
struct Shared<M>(Arc<DieselMigrations<M>>);

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
impl<DB: Backend, M: MigrationSource<DB>> MigrationSource<DB> for Shared<M> {
    fn migrations(&self) -> diesel::migration::Result<Vec<Box<dyn Migration<DB>>>> {
        self.0.source.migrations()
    }
}

#[cfg(feature = "postgres")]
impl<M> Migrations<crate::Postgres> for DieselMigrations<M>
where
    M: MigrationSource<diesel::pg::Pg> + Send + Sync + 'static,
{
    fn run(self: Arc<Self>, client: crate::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move {
            let given = Given::current()?;
            let conn = client.get().await?;
            given
                .run(move || {
                    let mut conn: AsyncConnectionWrapper<Object<diesel_async::AsyncPgConnection>> =
                        AsyncConnectionWrapper::from(conn);
                    conn.run_pending_migrations(Shared(self)).map(drop)
                })
                .await?
        })
    }
}

#[cfg(feature = "mysql")]
impl<M> Migrations<crate::MySql> for DieselMigrations<M>
where
    M: MigrationSource<diesel::mysql::Mysql> + Send + Sync + 'static,
{
    fn run(self: Arc<Self>, client: crate::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move {
            let given = Given::current()?;
            let conn = client.get().await?;
            given
                .run(move || {
                    let mut conn: AsyncConnectionWrapper<Object<diesel_async::AsyncMysqlConnection>> =
                        AsyncConnectionWrapper::from(conn);
                    conn.run_pending_migrations(Shared(self)).map(drop)
                })
                .await?
        })
    }
}

/// The wrapper's queries go through the connection's own `Given`, a second blocking-pool task that
/// the migration's thread waits on.
#[cfg(feature = "sqlite")]
impl<M> Migrations<crate::Sqlite> for DieselMigrations<M>
where
    M: MigrationSource<diesel::sqlite::Sqlite> + Send + Sync + 'static,
{
    fn run(self: Arc<Self>, client: crate::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        type Conn = diesel_async::sync_connection_wrapper::SyncConnectionWrapper<diesel::SqliteConnection, Given>;
        Box::pin(async move {
            let given = Given::current()?;
            let conn = client.get().await?;
            given
                .run(move || {
                    let mut conn: AsyncConnectionWrapper<Object<Conn>> = AsyncConnectionWrapper::from(conn);
                    conn.run_pending_migrations(Shared(self)).map(drop)
                })
                .await?
        })
    }
}
