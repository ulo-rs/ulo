use std::sync::Arc;
use std::time::Duration;

use ulo::{BoxError, BoxFuture};

use crate::error::MigrationFailed;
use crate::store::Store;

/// The key of the migration lock where the lock is a number: the first eight bytes of the SHA-256
/// of `"ulo-db/migrations"` as a big-endian `i64`, `0x30707e1b2c66b7b2`.
///
/// Postgres scopes advisory locks to the current database, so one constant serves every database.
/// SQLx's own key is `0x3d32ad9e` times the CRC-32 of the database name and this value is not a
/// multiple of it, so no database name makes the two meet: the locks nest, this one outside.
pub const MIGRATION_LOCK_KEY: i64 = 3_490_428_366_386_280_370;

/// A migrator for store `S`: SQLx's `Migrator`, `sea-orm-migration`'s, `diesel_migrations`' source.
///
/// A migrator that takes a lock of its own and one that takes none run alike: [`StoreModule`] holds
/// [`MigrationLock`]'s lock around [`run`](Self::run), so with the lock the second replica waits
/// where it would otherwise fail on the version table's key.
///
/// [`StoreModule`]: crate::StoreModule
pub trait Migrations<S: Store>: Send + Sync + 'static {
    /// Applies the pending migrations on `client`. A migration using `CREATE INDEX CONCURRENTLY` can
    /// deadlock against a session waiting on an advisory lock.
    fn run(self: Arc<Self>, client: S::Client) -> BoxFuture<'static, Result<(), BoxError>>;
}

/// The store side of the migration lock: the dedicated connection and the primitive. `ulo-db` owns
/// the key, the order, the bound and the error, and names no primitive.
///
/// Postgres: `pg_advisory_lock(MIGRATION_LOCK_KEY)`. MySQL: `GET_LOCK('ulo-db.migrate.' + database,
/// bound)`, the database name hashed when the lock name would pass 64 characters. SQLite: an OS lock
/// on `<file>.ulo-migrate.lock` through `std::fs::File::lock`, taken off the worker, and none for
/// an in-memory database. Each holds from a connection or file that releases it when it dies.
pub trait MigrationLock: Store {
    /// The held lock: the dedicated connection, or the locked file.
    type Guard: Send + 'static;

    /// Takes the lock, waiting at most `bound`.
    fn lock(client: &Self::Client, bound: Duration) -> BoxFuture<'static, Result<Self::Guard, BoxError>>;

    fn unlock(guard: Self::Guard) -> BoxFuture<'static, ()>;
}

/// Takes the lock, runs `migrations`, releases the lock, all under `bound`. A failure of any step,
/// the bound reached included, is a [`MigrationFailed`].
pub(crate) async fn run_locked<S: MigrationLock>(
    client: S::Client,
    migrations: Arc<dyn Migrations<S>>,
    bound: Duration,
) -> Result<(), MigrationFailed> {
    let _ = (client, migrations, bound);
    todo!()
}
