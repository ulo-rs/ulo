//! A replica that waits for another replica's migration lock until the bound fails retryably, and
//! a migration that itself fails stays permanent. The store is a fake whose lock the options
//! decide: `"held"` never frees, `"expires"` answers `LockTimedOut` at half the bound, `"free"` is
//! taken at once. The two waits have separate bounds: the lock's is `connect_within`, the
//! migrations' `migrate_within`, counted from the moment the lock is granted.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ulo::{
    App, Bound, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, Permanent, Signal, StartupError, Timer,
    is_permanent,
};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio};
use ulo_db::{LockTimedOut, MigrationLock, Migrations, Store, StoreModule};

struct Fake;

impl Store for Fake {
    const NAME: &'static str = "fake";
    type Options = String;
    type Client = String;

    fn create_connects(_: &String) -> bool {
        false
    }

    fn validate(_: &String) -> Result<(), Problems> {
        Ok(())
    }

    fn create(o: &String) -> BoxFuture<'static, Result<String, BoxError>> {
        let o = o.clone();
        Box::pin(async move { Ok(o) })
    }

    fn ping(_: &String) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async { Ok(()) })
    }

    fn close(_: String) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

impl MigrationLock for Fake {
    type Guard = ();

    fn lock(client: &String, bound: Duration) -> BoxFuture<'static, Result<(), BoxError>> {
        let mode = client.clone();
        Box::pin(async move {
            match mode.as_str() {
                "held" => std::future::pending().await,
                // Before `ulo-db`'s own bound passes, so the store's answer is what is classified.
                "expires" => {
                    Timer::sleep(&<OnTokio as Harness>::runtime(), bound / 2).await;
                    Err(LockTimedOut::new(bound).into())
                }
                // The lock's grant lands just inside the bound.
                "late" | "late-slow" | "late-bound-slow" => {
                    let wait = match mode.as_str() {
                        "late" => bound - Duration::from_millis(60),
                        "late-slow" => Duration::from_millis(300),
                        _ => Duration::from_millis(400),
                    };
                    Timer::sleep(&<OnTokio as Harness>::runtime(), wait).await;
                    Ok(())
                }
                // A connection lost between the ping and the lock: the store's classifier does not
                // call it permanent.
                "broken" => Err("the lock connection was reset".into()),
                // A refusal the store's classifier calls permanent, as it marks one for `ping`.
                "refused" => Err(Permanent::new("permission denied to take the lock").into()),
                _ => Ok(()),
            }
        })
    }

    fn unlock(_: ()) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

/// Fails, or runs past any bound, as the options of the store say after the lock is held.
struct Migrator;

impl Migrations<Fake> for Migrator {
    fn run(self: Arc<Self>, client: String) -> BoxFuture<'static, Result<(), BoxError>> {
        Box::pin(async move {
            match client.as_str() {
                "free-fails" => Err("migration 2 failed".into()),
                "free-panics" => panic!("a migration panicked"),
                "free-slow" => std::future::pending().await,
                "free-400" | "late" | "late-slow" | "late-bound-slow" => {
                    let wait = match client.as_str() {
                        "free-400" => 400,
                        "late" => 100,
                        _ => 300,
                    };
                    Timer::sleep(&<OnTokio as Harness>::runtime(), Duration::from_millis(wait)).await;
                    Ok(())
                }
                _ => Ok(()),
            }
        })
    }
}

struct Root(&'static str, Duration, Bound);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(
            StoreModule::<Fake>::from_options(self.0.to_owned())
                .connect_within(self.1)
                .migrate_within(self.2)
                .migrate(Migrator),
        );
    }
}

fn start(mode: &'static str, bound: Duration, migrate: Bound, hook: Option<Duration>) -> (Result<(), StartupError>, Duration) {
    <OnTokio as Harness>::block_on(async move {
        let mut builder = App::builder(Root(mode, bound, migrate)).runtime(<OnTokio as Harness>::runtime());
        if let Some(hook) = hook {
            builder = builder.hook_timeout(hook);
        }
        let app = builder.wire().expect("wires");
        let began = Instant::now();
        let result = app.connect().await;
        let took = began.elapsed();
        match result {
            Err(error) => (Err(error), took),
            Ok(app) => {
                let _ = app.close(Signal::new("test")).await;
                (Ok(()), took)
            }
        }
    })
}

fn connect(mode: &'static str, bound: Duration) -> (StartupError, Duration) {
    match start(mode, bound, Bound::Default, None) {
        (Err(error), took) => (error, took),
        (Ok(()), _) => panic!("{mode}: connected"),
    }
}

const BOUND: Duration = Duration::from_millis(600);

fn waited_to_the_bound(mode: &'static str) {
    let (error, took) = connect(mode, BOUND);
    assert!(!is_permanent(&error), "{mode}: a wait for another replica's lock is permanent: {error}");
    assert!(took >= BOUND, "{mode}: failed after {took:?}, before the {BOUND:?} bound");
    assert!(took < BOUND * 3, "{mode}: took {took:?}, far past the {BOUND:?} bound");
    assert!(error.to_string().contains("migration lock"), "{mode}: the failure does not name the lock: {error}");
}

#[test]
fn a_lock_that_never_frees_fails_retryably_at_the_bound() {
    waited_to_the_bound("held");
}

/// Planted break: `LockTimedOut` not recognised down the lock error's chain.
#[test]
fn a_lock_wait_that_expires_in_the_store_fails_retryably() {
    let (error, took) = connect("expires", BOUND);
    assert!(!is_permanent(&error), "a lock wait the store timed out is permanent: {error}");
    assert!(took < BOUND, "failed after {took:?}: the module's bound ended it, not the store's answer");
    assert!(error.to_string().contains("migration lock"), "the failure does not name the lock: {error}");
}

/// Planted break: every `MigrationFailed` of the lock step raised inside `Permanent`.
#[test]
fn a_lock_step_the_store_does_not_call_permanent_is_retryable() {
    let (error, took) = connect("broken", BOUND);
    assert!(!is_permanent(&error), "a lost lock connection is permanent: {error}");
    assert!(took < BOUND, "failed after {took:?}: the error waited for the bound");
}

/// Planted break: a lock-step error treated as retryable whatever the store marked.
#[test]
fn a_lock_step_the_store_calls_permanent_is_permanent() {
    let (error, _) = connect("refused", BOUND);
    assert!(is_permanent(&error), "a refusal the store marked permanent is retryable: {error}");
}

/// Planted break: the migrations counted against `connect_within` from the lock's request.
/// The grant lands 60 ms before the bound and the migration takes 100 ms, so a migration counted
/// against `connect_within` from the lock's request would be cut.
#[test]
fn a_lock_granted_at_the_connect_bound_still_migrates() {
    let (result, took) = start("late", BOUND, Bound::Default, None);
    result.unwrap_or_else(|error| panic!("a lock granted just inside the bound failed the start: {error}"));
    assert!(took >= BOUND - Duration::from_millis(100), "granted after {took:?}, not near the bound");
}

/// Planted break: the migrations bounded by `connect_within`.
#[test]
fn migrations_longer_than_the_connect_bound_succeed_when_migrate_within_allows() {
    let (result, took) = start("free-400", Duration::from_millis(200), Bound::After(Duration::from_secs(5)), None);
    result.unwrap_or_else(|error| panic!("a migration within `migrate_within` failed the start: {error}"));
    assert!(took >= Duration::from_millis(400), "finished after {took:?}, before the migration could");
}

/// Planted break: the hook left at `hook_timeout`, which would cut the lock's wait and the
/// migrations together at 250 ms.
#[test]
fn the_init_hook_covers_both_waits() {
    let (result, took) =
        start("late-slow", BOUND, Bound::After(Duration::from_secs(2)), Some(Duration::from_millis(250)));
    result.unwrap_or_else(|error| panic!("`hook_timeout` cut the migration step: {error}"));
    assert!(took >= Duration::from_millis(600), "finished after {took:?}");
}

/// The migrations' bound runs from the grant: a lock that arrives late does not shorten it.
/// Planted break: the migrations given only what is left of `connect_within`.
#[test]
fn migrate_within_counts_from_the_grant() {
    let (result, _) = start("late-bound-slow", BOUND, Bound::After(Duration::from_millis(500)), None);
    result.unwrap_or_else(|error| panic!("a 300 ms migration after a 400 ms lock wait failed a 500 ms bound: {error}"));
}

#[test]
fn a_failing_migration_stays_permanent() {
    let (error, _) = connect("free-fails", BOUND);
    assert!(is_permanent(&error), "a migration that failed is retryable: {error}");
}

/// A migration that panics is a migration that failed. Planted break: a panicked task read as
/// retryable.
#[test]
fn a_migration_that_panics_is_permanent() {
    let (error, _) = connect("free-panics", BOUND);
    assert!(is_permanent(&error), "a migration that panicked is retryable: {error}");
}

/// Planted break: running out of `migrate_within` classified as a lock wait.
#[test]
fn migrations_past_migrate_within_are_permanent() {
    let limit = Duration::from_millis(300);
    let (result, took) = start("free-slow", BOUND, Bound::After(limit), None);
    let error = result.expect_err("migrations that never finish succeeded");
    assert!(is_permanent(&error), "migrations that ran past `migrate_within` are retryable: {error}");
    assert!(took >= limit && took < BOUND, "failed after {took:?}, against a {limit:?} bound on the migrations");
}

#[test]
fn migrations_with_no_bound_are_not_cut_by_the_connect_bound() {
    let (result, took) = start("free-400", Duration::from_millis(150), Bound::Default, None);
    result.unwrap_or_else(|error| panic!("an unbounded migration was cut: {error}"));
    assert!(took >= Duration::from_millis(400));
}
