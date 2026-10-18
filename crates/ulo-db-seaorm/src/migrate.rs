use std::any::Any;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use sea_orm::DatabaseConnection;
use sea_orm_migration::MigratorTrait;
use ulo::{BoxError, BoxFuture};
use ulo_db::{MigrationLock, Migrations};

use crate::store::SeaOrm;

/// A `sea-orm-migration` migrator `M` as the store's migrations. `sea-orm-migration` takes no lock,
/// so `ulo-db`'s lock around [`run`](Migrations::run) is what keeps two replicas from applying a
/// migration body at once.
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
        let _ = (self, client);
        todo!()
    }
}

/// The held lock: a connection from the pool SeaORM holds, with the primitive of the engine it
/// speaks, or the locked sidecar file for SQLite.
pub struct SeaOrmLock {
    held: Box<dyn Any + Send>,
}

impl MigrationLock for SeaOrm {
    type Guard = SeaOrmLock;

    fn lock(client: &DatabaseConnection, bound: Duration) -> BoxFuture<'static, Result<SeaOrmLock, BoxError>> {
        let _ = (client, bound);
        todo!()
    }

    fn unlock(guard: SeaOrmLock) -> BoxFuture<'static, ()> {
        let _ = guard.held;
        todo!()
    }
}
