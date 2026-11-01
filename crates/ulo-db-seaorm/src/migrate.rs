use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
#[cfg(any(feature = "postgres", feature = "mysql", feature = "sqlite"))]
use sea_orm::DbBackend;
use sea_orm_migration::MigratorTrait;
use ulo::{BoxError, BoxFuture};
use ulo_db::{MigrationLock, Migrations};
#[cfg(feature = "mysql")]
use ulo_db_sqlx::{MySql, MySqlLock};
#[cfg(feature = "postgres")]
use ulo_db_sqlx::{PgLock, Postgres};
#[cfg(feature = "sqlite")]
use ulo_db_sqlx::{Sqlite, SqliteLock};

use crate::store::SeaOrm;

/// A `sea-orm-migration` migrator `M` as the store's migrations. `sea-orm-migration` takes no lock,
/// so `ulo-db`'s lock around [`run`](Migrations::run) is what keeps two replicas from applying a
/// migration body at once.
///
/// On Postgres and MySQL the lock holds one pooled connection for the length of the run, so the
/// pool needs `max_connections` of at least 2. A migration using `CREATE INDEX CONCURRENTLY` can
/// deadlock against a session waiting on the advisory lock.
pub struct SeaOrmMigrations<M> {
    _migrator: PhantomData<fn() -> M>,
}

impl<M> SeaOrmMigrations<M> {
    pub fn new() -> Self {
        SeaOrmMigrations { _migrator: PhantomData }
    }
}

impl<M> Default for SeaOrmMigrations<M> {
    fn default() -> Self {
        SeaOrmMigrations::new()
    }
}

impl<M: MigratorTrait + 'static> Migrations<SeaOrm> for SeaOrmMigrations<M> {
    fn run(self: Arc<Self>, client: DatabaseConnection) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move { M::up(&client, None).await.map_err(|error| -> BoxError { Box::new(error) }) })
    }
}

/// The held lock: the one `ulo-db-sqlx` takes for the engine, on the SQLx pool SeaORM holds. That
/// is the same SQLx, since `ulo-db-sqlx` and SeaORM share the workspace's entry, so the pool
/// SeaORM hands out is the pool type those locks take.
pub struct SeaOrmLock {
    held: Held,
}

enum Held {
    #[cfg(feature = "postgres")]
    Postgres(PgLock),
    #[cfg(feature = "mysql")]
    MySql(MySqlLock),
    #[cfg(feature = "sqlite")]
    Sqlite(SqliteLock),
}

impl MigrationLock for SeaOrm {
    type Guard = SeaOrmLock;

    // `bound` is read only by the engine arms, which each need an engine feature.
    #[cfg_attr(not(any(feature = "postgres", feature = "mysql", feature = "sqlite")), allow(unused_variables))]
    fn lock(client: &DatabaseConnection, bound: Duration) -> BoxFuture<'static, Result<SeaOrmLock, BoxError>> {
        match client.get_database_backend() {
            #[cfg(feature = "postgres")]
            DbBackend::Postgres => {
                let held = <Postgres as MigrationLock>::lock(client.get_postgres_connection_pool(), bound);
                Box::pin(async move { held.await.map(|guard| SeaOrmLock { held: Held::Postgres(guard) }) })
            }
            #[cfg(feature = "mysql")]
            DbBackend::MySql => {
                let held = <MySql as MigrationLock>::lock(client.get_mysql_connection_pool(), bound);
                Box::pin(async move { held.await.map(|guard| SeaOrmLock { held: Held::MySql(guard) }) })
            }
            #[cfg(feature = "sqlite")]
            DbBackend::Sqlite => {
                let held = <Sqlite as MigrationLock>::lock(client.get_sqlite_connection_pool(), bound);
                Box::pin(async move { held.await.map(|guard| SeaOrmLock { held: Held::Sqlite(guard) }) })
            }
            other => {
                let message = format!("ulo-db-seaorm has no migration lock compiled in for {other:?}");
                Box::pin(std::future::ready(Err::<SeaOrmLock, BoxError>(message.into())))
            }
        }
    }

    fn unlock(guard: SeaOrmLock) -> BoxFuture<'static, ()> {
        match guard.held {
            #[cfg(feature = "postgres")]
            Held::Postgres(lock) => <Postgres as MigrationLock>::unlock(lock),
            #[cfg(feature = "mysql")]
            Held::MySql(lock) => <MySql as MigrationLock>::unlock(lock),
            #[cfg(feature = "sqlite")]
            Held::Sqlite(lock) => <Sqlite as MigrationLock>::unlock(lock),
        }
    }
}
