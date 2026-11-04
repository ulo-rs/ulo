use std::any::{TypeId, type_name};
use std::error::Error;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use ulo::{BoxError, BoxFuture, Cut, Runtime, RuntimeShutDown, spawn_with};

use crate::connect::{Attached, carries_permanent, within};
use crate::error::{LockFailed, LockTimedOut, MigrationFailed, MigrationTimedOut, Retry, TaskFailed};
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

    /// Which migrations these are, for comparing two imports of one store module: the `migrate`
    /// setting is this identity's text, so two imports naming one migrator are one module and
    /// two naming different migrators are refused at `wire()` as
    /// `ulo::WiringError::DifferingSettings`.
    ///
    /// `None`, the default, compares the `.migrate(..)` call that set the migrator instead, its
    /// file, line and column. That refuses one migrator written at two calls, and matches two
    /// migrators passed through one helper function's single call, the second import then
    /// running the first's migrations.
    fn identity(&self) -> Option<MigratorIdentity> {
        None
    }
}

/// What a migrator is, as [`Migrations::identity`] answers it: a `static`'s address or a type,
/// each written with the type's name. Two identities are equal when both name one `static`, or
/// both one type.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MigratorIdentity {
    of: Of,
    name: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Of {
    Address(usize),
    Type(TypeId),
}

impl MigratorIdentity {
    /// A migrator held in a `static`, compared by its address: `&MIGRATOR` passed at two calls is
    /// one migrator, and two `static`s are two, whatever they hold. A promoted constant written
    /// inline, `&CONST` or `&sqlx::migrate!()`, has whatever address the compiler gives it:
    /// rustc gives two occurrences of one constant one address in debug and release builds, and
    /// in a release build a `static` can share the address of an equal inline constant. The
    /// compiler merges two values only when their bytes are identical, which for a migrator means
    /// the same migrations, so a merged address never names two different migrators. The merge
    /// itself is not promised, so a `static` is what makes one migrator compare equal to itself.
    ///
    /// `T` is sized: a slice or a trait object would be compared by its data address alone, so
    /// two slices starting at one element would compare equal. A zero-sized value has no address
    /// of its own; [`of_type`](Self::of_type) names one.
    ///
    /// ```compile_fail,E0277
    /// static MIGRATIONS: [u8; 2] = [1, 2];
    /// ulo_db::MigratorIdentity::of_static::<[u8]>(&MIGRATIONS[..1]);
    /// ```
    pub fn of_static<T>(migrator: &'static T) -> Self {
        let address = (migrator as *const T).cast::<()>() as usize;
        MigratorIdentity { of: Of::Address(address), name: type_name::<T>() }
    }

    /// A migrator that is a type, compared by its `TypeId`: two values of one type are one
    /// migrator.
    pub fn of_type<T: ?Sized + 'static>() -> Self {
        MigratorIdentity { of: Of::Type(TypeId::of::<T>()), name: type_name::<T>() }
    }
}

/// `{name} at {address}` for a `static`, and `{name}` followed by its `TypeId` for a type: the
/// `migrate` setting compares by this text, and a type's name alone is shared by one crate's
/// type at two versions.
impl fmt::Display for MigratorIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.of {
            Of::Address(address) => write!(f, "`{}` at {address:#x}", self.name),
            Of::Type(id) => write!(f, "`{}` ({id:?})", self.name),
        }
    }
}

impl fmt::Debug for MigratorIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
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

    /// Takes the lock, waiting at most `bound`: `connect_within`, the wait's own bound. A wait that
    /// runs out answers [`LockTimedOut`]. Any other error is classified as [`Store::ping`]'s are:
    /// one only the operator can mend, such as no permission to take the lock, is returned inside
    /// `ulo::Permanent`, and any other, a connection lost or a server restarted, bare, which
    /// `ulo-db` reads as retryable.
    fn lock(client: &Self::Client, bound: Duration) -> BoxFuture<'static, Result<Self::Guard, BoxError>>;

    fn unlock(guard: Self::Guard) -> BoxFuture<'static, ()>;
}

/// The migrations a module runs, with the lock protocol instantiated for its store where
/// `S: MigrationLock` is known, which `Module::register` is not.
pub(crate) struct Migrate<S: Store> {
    migrations: Arc<dyn Migrations<S>>,
    pub(crate) identity: Option<MigratorIdentity>,
    run: RunLocked<S>,
}

/// The two waits of a migration step, each with its own bound.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Bounds {
    /// The wait for the lock, counted from the request.
    pub(crate) lock: Duration,
    /// The migrations, counted from the moment the lock is held. `None` is unbounded.
    pub(crate) migrations: Option<Duration>,
}

/// How long the release may take before the guard is dropped instead, which releases the lock too.
const UNLOCK_BOUND: Duration = Duration::from_secs(5);

type RunLocked<S> = fn(
    Arc<dyn Runtime>,
    <S as Store>::Client,
    Arc<dyn Migrations<S>>,
    Bounds,
) -> BoxFuture<'static, Result<(), MigrationFailed>>;

impl<S: Store> Migrate<S> {
    pub(crate) fn new<M: Migrations<S>>(migrations: M) -> Self
    where
        S: MigrationLock,
    {
        let identity = migrations.identity();
        Migrate { migrations: Arc::new(migrations), identity, run: run_boxed::<S> }
    }

    /// The lock, the migrations and the release on `client`, under `bounds`.
    pub(crate) fn run(
        &self,
        runtime: Arc<dyn Runtime>,
        client: S::Client,
        bounds: Bounds,
    ) -> BoxFuture<'static, Result<(), MigrationFailed>> {
        (self.run)(runtime, client, Arc::clone(&self.migrations), bounds)
    }
}

impl<S: Store> Clone for Migrate<S> {
    fn clone(&self) -> Self {
        Migrate { migrations: Arc::clone(&self.migrations), identity: self.identity, run: self.run }
    }
}

fn run_boxed<S: MigrationLock>(
    runtime: Arc<dyn Runtime>,
    client: S::Client,
    migrations: Arc<dyn Migrations<S>>,
    bounds: Bounds,
) -> BoxFuture<'static, Result<(), MigrationFailed>> {
    Box::pin(run_locked::<S>(runtime, client, migrations, bounds))
}

/// Takes the lock, runs `migrations`, releases the lock. A failure of any step is a
/// [`MigrationFailed`], which says whether retrying the start can mend it:
///
/// - The wait for the lock reaching `bounds.lock`, whether the store's own wait ran out
///   ([`LockTimedOut`]) or this function's clock did: retryable, another replica is migrating.
/// - Any other error of the lock step, a lost connection or an SQL refusal: the store's own
///   classification decides, as it does for `ping`. An error carrying `ulo::Permanent` is not
///   retryable and any other is.
/// - Everything after the lock is held, a failing migration and `bounds.migrations` passing among
///   them: not retryable.
/// - Either bound refused by the runtime, having shut down: a [`RuntimeShutDown`], not retryable.
///
/// The steps run as one task on the app's runtime, so whatever the migrator spawns runs there. Both
/// bounds are timed on the app's clock inside the task, the second starting when the lock is held,
/// and dropping the returned future aborts the task, which drops the guard and releases the lock.
pub(crate) async fn run_locked<S: MigrationLock>(
    runtime: Arc<dyn Runtime>,
    client: S::Client,
    migrations: Arc<dyn Migrations<S>>,
    bounds: Bounds,
) -> Result<(), MigrationFailed> {
    let clock = Arc::clone(&runtime);
    let steps = async move {
        let waiting: BoxFuture<'static, _> = Box::pin(S::lock(&client, bounds.lock));
        let guard = match ulo::timeout(&*clock, bounds.lock, waiting).await {
            Ok(Ok(guard)) => guard,
            Ok(Err(error)) => return Err(BoxError::from(LockFailed(error))),
            Err(Cut::Expired) => return Err(LockTimedOut::new(bounds.lock).into()),
            Err(Cut::Refused(refused)) => return Err(RuntimeShutDown::new(refused).into()),
        };
        let running = migrations.run(client);
        let ran: Result<(), BoxError> = match bounds.migrations {
            None => running.await,
            Some(limit) => match ulo::timeout(&*clock, limit, running).await {
                Ok(ran) => ran,
                // Dropped where it stands: the guard releases the lock on drop.
                Err(Cut::Expired) => return Err(MigrationTimedOut { bound: limit }.into()),
                Err(Cut::Refused(refused)) => return Err(RuntimeShutDown::new(refused).into()),
            },
        };
        let unlocking: BoxFuture<'static, ()> = S::unlock(guard);
        let _ = within(&*clock, UNLOCK_BOUND, unlocking).await;
        ran
    };
    let task = Attached(spawn_with(&*runtime, steps));
    let (source, retry) = match task.await {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(error)) => {
            let retry = if error.is::<LockTimedOut>() {
                Retry::LockWait
            } else if let Some(failed) = error.downcast_ref::<LockFailed>() {
                if lock_timed_out(&*failed.0) {
                    Retry::LockWait
                } else if carries_permanent(&*failed.0) {
                    Retry::No
                } else {
                    Retry::LockStep
                }
            } else {
                Retry::No
            };
            (error, retry)
        }
        Err(end) => (TaskFailed::new(S::NAME, "migration", end).into(), Retry::No),
    };
    Err(MigrationFailed::new(S::NAME, source, retry))
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
