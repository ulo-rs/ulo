//! A replica that waits for another replica's migration lock until the bound fails retryably, and
//! a migration that itself fails stays permanent. The store is a fake whose lock the options
//! decide: `"held"` never frees, `"expires"` answers `LockTimedOut` at the bound, `"free"` is
//! taken at once.

use std::sync::Arc;
use std::time::{Duration, Instant};

use ulo::{App, BoxError, BoxFuture, Module, ModuleDef, ModuleIdentity, StartupError, Timer, is_permanent};
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
                "expires" => {
                    Timer::sleep(&<OnTokio as Harness>::runtime(), bound).await;
                    Err(LockTimedOut::new(bound).into())
                }
                "broken" => Err("the lock connection was refused".into()),
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
                "free-slow" => std::future::pending().await,
                _ => Ok(()),
            }
        })
    }
}

struct Root(&'static str, Duration);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(StoreModule::<Fake>::from_options(self.0.to_owned()).connect_within(self.1).migrate(Migrator));
    }
}

fn connect(mode: &'static str, bound: Duration) -> (StartupError, Duration) {
    <OnTokio as Harness>::block_on(async move {
        let app = App::builder(Root(mode, bound)).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
        let began = Instant::now();
        match app.connect().await {
            Err(error) => (error, began.elapsed()),
            Ok(_) => panic!("{mode}: connected"),
        }
    })
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

#[test]
fn a_lock_wait_that_expires_in_the_store_fails_retryably() {
    waited_to_the_bound("expires");
}

#[test]
fn a_lock_that_cannot_be_taken_stays_permanent() {
    let (error, _) = connect("broken", BOUND);
    assert!(is_permanent(&error), "a lock that failed for another reason is retryable: {error}");
}

#[test]
fn a_failing_migration_stays_permanent() {
    let (error, _) = connect("free-fails", BOUND);
    assert!(is_permanent(&error), "a migration that failed is retryable: {error}");
}

#[test]
fn migrations_past_the_bound_stay_permanent() {
    let (error, took) = connect("free-slow", BOUND);
    assert!(is_permanent(&error), "migrations that ran past the bound are retryable: {error}");
    assert!(took >= BOUND, "failed after {took:?}, before the bound");
}
