use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use ulo::{BoxError, BoxFuture, Runtime, spawn_with};

use crate::connect::{Attached, within};
use crate::error::{LockFailed, LockTimedOut, MigrationFailed, MigrationTimedOut, TaskFailed};
use crate::store::Store;

/// How deep `lock_timed_out` walks a `source()` chain.
const MAX_DEPTH: usize = 32;

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
///
/// `lock` and `unlock` run inside one task on the app's runtime, `lock` first, then the migrator,
/// then `unlock` whether the migrator succeeded or not. When the module's bound passes first the
/// task is aborted and the guard dropped where it stands, so a guard releases the lock on drop as
/// well as through `unlock`.
pub trait MigrationLock: Store {
    /// The held lock: the dedicated connection, or the locked file.
    type Guard: Send + 'static;

    /// Takes the lock, waiting at most `bound`.
    fn lock(client: &Self::Client, bound: Duration) -> BoxFuture<'static, Result<Self::Guard, BoxError>>;

    fn unlock(guard: Self::Guard) -> BoxFuture<'static, ()>;
}

/// The migrations a module runs, with the lock protocol instantiated for its store where
/// `S: MigrationLock` is known, which `Module::register` is not.
pub(crate) struct Migrate<S: Store> {
    migrations: Arc<dyn Migrations<S>>,
    run: RunLocked<S>,
}

type RunLocked<S> =
    fn(Arc<dyn Runtime>, <S as Store>::Client, Arc<dyn Migrations<S>>, Duration) -> BoxFuture<'static, Result<(), MigrationFailed>>;

impl<S: Store> Migrate<S> {
    pub(crate) fn new<M: Migrations<S>>(migrations: M) -> Self
    where
        S: MigrationLock,
    {
        Migrate { migrations: Arc::new(migrations), run: run_boxed::<S> }
    }

    /// The lock, the migrations and the release on `client`, under `bound`.
    pub(crate) fn run(&self, runtime: Arc<dyn Runtime>, client: S::Client, bound: Duration) -> BoxFuture<'static, Result<(), MigrationFailed>> {
        (self.run)(runtime, client, Arc::clone(&self.migrations), bound)
    }
}

impl<S: Store> Clone for Migrate<S> {
    fn clone(&self) -> Self {
        Migrate { migrations: Arc::clone(&self.migrations), run: self.run }
    }
}

fn run_boxed<S: MigrationLock>(
    runtime: Arc<dyn Runtime>,
    client: S::Client,
    migrations: Arc<dyn Migrations<S>>,
    bound: Duration,
) -> BoxFuture<'static, Result<(), MigrationFailed>> {
    Box::pin(run_locked::<S>(runtime, client, migrations, bound))
}

/// Takes the lock, runs `migrations`, releases the lock, all under `bound`. A failure of any step,
/// the bound reached included, is a [`MigrationFailed`].
///
/// Only one failure is retryable, and `MigrationFailed::waited_for_lock` names it: the wait for the
/// lock reaching the bound, whether the store's own wait ran out ([`LockTimedOut`]) or this
/// function's bound passed before the lock was held. Every other failure, a migration that fails
/// or a bound passed once the lock was held among them, is not.
///
/// The three steps run as one task on the app's runtime, so whatever the migrator spawns runs
/// there; the bound is timed on the app's clock outside that task and aborts it on expiry.
pub(crate) async fn run_locked<S: MigrationLock>(
    runtime: Arc<dyn Runtime>,
    client: S::Client,
    migrations: Arc<dyn Migrations<S>>,
    bound: Duration,
) -> Result<(), MigrationFailed> {
    let held = Arc::new(AtomicBool::new(false));
    let steps_held = Arc::clone(&held);
    let steps = async move {
        let guard = match S::lock(&client, bound).await {
            Ok(guard) => guard,
            Err(error) => return Err(BoxError::from(LockFailed(error))),
        };
        steps_held.store(true, Ordering::SeqCst);
        let ran: Result<(), BoxError> = migrations.run(client).await;
        S::unlock(guard).await;
        ran
    };
    let task = Attached(spawn_with(&*runtime, steps));
    let (source, lock_wait): (BoxError, bool) = match within(&*runtime, bound, task).await {
        Some(Ok(Ok(()))) => return Ok(()),
        Some(Ok(Err(error))) => {
            let timed_out = error.downcast_ref::<LockFailed>().is_some_and(|failed| lock_timed_out(&*failed.0));
            (error, timed_out)
        }
        Some(Err(end)) => (TaskFailed::new(S::NAME, "migration", end).into(), false),
        None if held.load(Ordering::SeqCst) => (MigrationTimedOut { bound }.into(), false),
        None => (LockTimedOut::new(bound).into(), true),
    };
    Err(MigrationFailed::new(S::NAME, source, lock_wait))
}

/// Whether `error` or anything down its `source()` chain is a [`LockTimedOut`].
fn lock_timed_out(error: &(dyn Error + 'static)) -> bool {
    let mut next = Some(error);
    for _ in 0..MAX_DEPTH {
        match next {
            Some(e) if e.is::<LockTimedOut>() => return true,
            Some(e) => next = e.source(),
            None => return false,
        }
    }
    false
}
