//! `diesel_migrations` through `AsyncConnectionWrapper`, which blocks on futures and panics on a
//! tokio worker. The closure runs through the same `Given` on the given runtime's blocking pool,
//! where `AsyncConnectionWrapper`'s tokio `BlockOn` reads `Handle::try_current()` and finds that
//! runtime. `AsyncMigrationHarness` uses `block_in_place`, which panics on a current-thread runtime,
//! and is not used. `diesel_migrations` takes no lock, so `ulo-db`'s lock wraps the run.

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use std::sync::Arc;

#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use diesel::migration::MigrationSource;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo::{BoxError, BoxFuture};
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use ulo_db::Migrations;

/// A `diesel_migrations` source, `embed_migrations!()`'s value or a directory's.
pub struct DieselMigrations<M> {
    #[allow(dead_code)]
    pub(crate) source: M,
}

impl<M> DieselMigrations<M> {
    pub fn new(source: M) -> Self {
        DieselMigrations { source }
    }
}

#[cfg(feature = "postgres")]
impl<M> Migrations<crate::Postgres> for DieselMigrations<M>
where
    M: MigrationSource<diesel::pg::Pg> + Send + Sync + 'static,
{
    fn run(self: Arc<Self>, client: crate::PgPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (self, client);
        todo!()
    }
}

#[cfg(feature = "mysql")]
impl<M> Migrations<crate::MySql> for DieselMigrations<M>
where
    M: MigrationSource<diesel::mysql::Mysql> + Send + Sync + 'static,
{
    fn run(self: Arc<Self>, client: crate::MySqlPool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (self, client);
        todo!()
    }
}

#[cfg(feature = "sqlite")]
impl<M> Migrations<crate::Sqlite> for DieselMigrations<M>
where
    M: MigrationSource<diesel::sqlite::Sqlite> + Send + Sync + 'static,
{
    fn run(self: Arc<Self>, client: crate::SqlitePool) -> BoxFuture<'static, Result<(), BoxError>> {
        let _ = (self, client);
        todo!()
    }
}
