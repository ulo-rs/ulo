//! `connect_within` and `migrate_within` on a runtime that refuses to time them, having shut down.
//! Each bound's expiry is reported to the deployer, so a refused bound reports the refusal, a
//! `RuntimeShutDown` inside `Permanent`, and not the bound passing. Each site has a control on a
//! clock that keeps time, where the bound passes and is reported as before.
//!
//! The store is a fake whose options name the scenario and key its clock: `create-*` connects in
//! `create`, which never answers; `lock-*` waits on a lock that never frees; `migrate-*` takes the
//! lock and runs migrations that never finish. A `lock-*` store's ping refuses `CONNECT_WITHIN` on
//! the scenario's clock as it passes, so the readiness check, whose bound has that length too, is
//! armed before the refusal and the lock's wait after it.

use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::future::{Either, join, select};
use ulo::{
    App, Bound, BoxError, BoxFuture, ConnectError, FailureReason, Limit, Module, ModuleDef, ModuleIdentity, Permanent,
    RUNTIME_SHUT_DOWN, Redacted, Refused, RuntimeShutDown, StartupError, Timer, is_permanent,
};
use ulo_config::Problems;
use ulo_conformance_support::{Harness, OnTokio, ScriptedTimer};
use ulo_db::{LockTimedOut, MigrationFailed, MigrationLock, Migrations, Store, StoreModule};

const CONNECT_WITHIN: Duration = Duration::from_secs(41);
const MIGRATE_WITHIN: Duration = Duration::from_secs(43);
/// The init hook's own bound over the migrations: both waits and the module's ten-second margin.
const HOOK_BOUND: Duration = Duration::from_secs(41 + 43 + 10);

/// How long a scenario may take on the real clock before it is failed rather than left to hang.
const CAP: Duration = Duration::from_secs(10);

/// Each scenario's clock, by its options, and whether its store reached the wait under test.
static SCENARIOS: Mutex<Vec<(String, ScriptedTimer, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

fn scenario(mode: &str) -> (ScriptedTimer, Arc<AtomicBool>) {
    let mut scenarios = SCENARIOS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, clock, reached)) = scenarios.iter().find(|(m, ..)| m == mode) {
        return (clock.clone(), Arc::clone(reached));
    }
    let made = (ScriptedTimer::new(), Arc::new(AtomicBool::new(false)));
    scenarios.push((mode.to_owned(), made.0.clone(), Arc::clone(&made.1)));
    made
}

fn reached(mode: &str) {
    scenario(mode).1.store(true, Ordering::SeqCst);
}

/// Each `*-late` scenario's switch: once set, its clock refuses the late length only once it is
/// due (see [`Late`]).
static LATE: Mutex<Vec<(String, Arc<AtomicBool>)>> = Mutex::new(Vec::new());

fn late_from(mode: &str) -> Arc<AtomicBool> {
    let mut lates = LATE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((_, armed)) = lates.iter().find(|(m, _)| m == mode) {
        return Arc::clone(armed);
    }
    let armed = Arc::new(AtomicBool::new(false));
    lates.push((mode.to_owned(), Arc::clone(&armed)));
    armed
}

/// A scenario's [`ScriptedTimer`] whose wait of `late`, once `armed`, is refused only once it is
/// due: the clock moves past `late` as the wait is asked, as a runtime blocked past a deadline and
/// then shut down leaves it.
struct Late {
    clock: ScriptedTimer,
    late: Duration,
    armed: Arc<AtomicBool>,
}

impl Timer for Late {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> std::time::Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        if d == self.late && self.armed.load(Ordering::SeqCst) {
            self.clock.advance(d);
            return Box::pin(std::future::ready(Err(Refused)));
        }
        self.clock.try_sleep(d)
    }
}

struct Fake;

impl Store for Fake {
    const NAME: &'static str = "fake";
    type Options = String;
    type Client = String;

    fn create_connects(o: &String) -> bool {
        o.starts_with("create")
    }

    fn validate(_: &String) -> Result<(), Problems> {
        Ok(())
    }

    fn create(o: &String) -> BoxFuture<'static, Result<String, BoxError>> {
        let o = o.clone();
        Box::pin(async move {
            match o.as_str() {
                "create-fails" => Err(NoAnswer.into()),
                m if m.starts_with("create") => std::future::pending().await,
                _ => Ok(o),
            }
        })
    }

    fn ping(client: &String) -> BoxFuture<'static, Result<(), BoxError>> {
        if client == "lock-refused" {
            scenario(client).0.refuse(CONNECT_WITHIN);
        }
        if client == "lock-late" {
            late_from(client).store(true, Ordering::SeqCst);
        }
        Box::pin(async { Ok(()) })
    }

    fn close(_: String) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

impl MigrationLock for Fake {
    type Guard = ();

    fn lock(client: &String, _bound: Duration) -> BoxFuture<'static, Result<(), BoxError>> {
        let mode = client.clone();
        reached(&mode);
        Box::pin(async move {
            if mode.starts_with("lock") {
                std::future::pending::<()>().await;
            }
            Ok(())
        })
    }

    fn unlock(_: ()) -> BoxFuture<'static, ()> {
        Box::pin(async {})
    }
}

struct Migrator;

impl Migrations<Fake> for Migrator {
    fn run(self: Arc<Self>, client: String) -> BoxFuture<'static, Result<(), BoxError>> {
        reached(&client);
        Box::pin(std::future::pending())
    }
}

#[derive(Debug)]
struct NoAnswer;

impl fmt::Display for NoAnswer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("no answer")
    }
}

impl Error for NoAnswer {}

struct Root(&'static str);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_value(&self.0)
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let module = StoreModule::<Fake>::from_options(self.0.to_owned()).connect_within(CONNECT_WITHIN);
        m.import(match self.0 {
            "create-fails" => module.retries(2).backoff(Duration::from_millis(1)),
            m if m.starts_with("lock") => module.migrate(Migrator),
            m if m.starts_with("migrate") => module.migrate_within(Bound::After(MIGRATE_WITHIN)).migrate(Migrator),
            _ => module,
        });
    }
}

/// `fut` on the harness, failed once `CAP` passes on the real clock.
fn capped<T>(what: &str, fut: impl Future<Output = T>) -> T {
    <OnTokio as Harness>::block_on(async {
        match select(Box::pin(fut), Timer::sleep(&<OnTokio as Harness>::runtime(), CAP)).await {
            Either::Left((done, _)) => done,
            Either::Right(..) => panic!("{what}: still running after {CAP:?}"),
        }
    })
}

/// The start of an app over `mode`'s store on the scenario's clock, with `drive` run beside it.
fn start_with(mode: &'static str, drive: impl Future<Output = ()>) -> StartupError {
    let clock = scenario(mode).0;
    capped(mode, async move {
        let app = App::builder(Root(mode)).runtime(<OnTokio as Harness>::runtime()).timer(clock).wire().expect("wires");
        let (connected, ()) = join(app.connect(), drive).await;
        connected.err().unwrap_or_else(|| panic!("{mode}: connected"))
    })
}

fn start(mode: &'static str) -> StartupError {
    start_with(mode, async {})
}

/// The start of an app over `mode`'s store on its clock wrapped in [`Late`] for `late`.
fn start_late(mode: &'static str, late: Duration) -> StartupError {
    let timer = Late { clock: scenario(mode).0, late, armed: late_from(mode) };
    capped(mode, async move {
        let app = App::builder(Root(mode)).runtime(<OnTokio as Harness>::runtime()).timer(timer).wire().expect("wires");
        app.connect().await.err().unwrap_or_else(|| panic!("{mode}: connected"))
    })
}

/// Advances `mode`'s clock by `by` once its store has reached the wait under test and the one
/// sleep of length `by` it then arms has seen the clock short of its end.
async fn advance_once_waiting(mode: &'static str, by: Duration) {
    let (clock, reached) = scenario(mode);
    let runtime = <OnTokio as Harness>::runtime();
    while !(reached.load(Ordering::SeqCst) && clock.live(by) == 1 && clock.pending_at(by) == Some(clock.elapsed())) {
        Timer::sleep(&runtime, Duration::from_millis(1)).await;
    }
    clock.advance(by);
}

/// The reason of a connect failure and the error it stores, if it stores one.
fn reason(error: &StartupError) -> &FailureReason {
    match error {
        StartupError::Connect(
            ConnectError::Construct { reason, .. } | ConnectError::Readiness { reason, .. } | ConnectError::Hook { reason, .. },
        ) => reason,
        other => panic!("the start failed outside `connect`: {other:?}"),
    }
}

fn stored(error: &StartupError) -> &Redacted {
    match reason(error) {
        FailureReason::Errored(stored) => stored,
        other => panic!("the start failed without an error: {other:?}"),
    }
}

/// The first `E` down the chain from `from`, itself included.
fn found<'a, E: Error + 'static>(from: &'a (dyn Error + 'static)) -> Option<&'a E> {
    let mut next = Some(from);
    for _ in 0..8 {
        let error = next?;
        if let Some(hit) = error.downcast_ref::<E>() {
            return Some(hit);
        }
        next = error.source();
    }
    None
}

/// The `RuntimeShutDown` inside the `Permanent` a start failed with, whose source is the
/// runtime's `Refused`.
fn refusal(error: &StartupError) -> &(dyn Error + 'static) {
    assert!(is_permanent(error), "a start ended on a refused bound is retryable: {error}");
    let permanent: &(dyn Error + 'static) =
        stored(error).downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the start's error is not `Permanent`: {error:?}"));
    let ended = found::<RuntimeShutDown>(permanent).unwrap_or_else(|| panic!("no `RuntimeShutDown` under the `Permanent`: {error:?}"));
    assert_eq!(ended.reason(), RUNTIME_SHUT_DOWN);
    assert!(ended.source().is_some_and(|source| source.is::<Refused>()), "the refusal's source is not the runtime's `Refused`: {ended:?}");
    permanent
}

/// Planted break: the loop around `create` bounded by the construction bound alone, whose `sleep`
/// a runtime that has shut down ends at once, so the start reads as `connect_within` passing.
#[test]
fn a_refused_connect_bound_fails_the_create_loop_with_the_refusal() {
    scenario("create-refused").0.refuse(CONNECT_WITHIN);
    let error = start("create-refused");
    assert!(matches!(error, StartupError::Connect(ConnectError::Construct { .. })), "{error:?}");
    refusal(&error);
}

/// The control: on a clock that keeps time, the loop around `create` reaching `connect_within`
/// still fails the construction as the bound passing, retryable.
#[test]
fn a_connect_bound_that_passes_still_times_the_create_loop_out() {
    let error = start_with("create-expires", async {
        let (clock, runtime) = (scenario("create-expires").0, <OnTokio as Harness>::runtime());
        // The construction bound and the loop's own wait for a refusal, both of `CONNECT_WITHIN`.
        while !(clock.live(CONNECT_WITHIN) == 2 && clock.pending_at(CONNECT_WITHIN) == Some(clock.elapsed())) {
            Timer::sleep(&runtime, Duration::from_millis(1)).await;
        }
        clock.advance(CONNECT_WITHIN);
    });
    assert!(!is_permanent(&error), "a connect bound that passed is permanent: {error}");
    let StartupError::Connect(ConnectError::Construct { reason: FailureReason::TimedOut { after, limit }, .. }) = &error else {
        panic!("the loop did not time out at the construction bound: {error:?}");
    };
    assert_eq!((*after, *limit), (CONNECT_WITHIN, Limit::Item));
}

/// The control for the loop's pacing: retries that run out on a clock that keeps time end with the
/// last attempt's error, retryable, and no `RuntimeShutDown`.
#[test]
fn a_create_loop_out_of_retries_ends_with_the_last_error() {
    let error = capped("create-fails", async {
        let app = App::builder(Root("create-fails")).runtime(<OnTokio as Harness>::runtime()).wire().expect("wires");
        app.connect().await.err().expect("connected")
    });
    assert!(!is_permanent(&error), "a loop out of retries is permanent: {error}");
    assert!(stored(&error).downcast_ref::<NoAnswer>().is_some(), "the loop did not end with the last attempt's error: {error:?}");
}

/// Planted break: the lock's wait timed on `sleep`, so the refusal reads as another replica
/// holding the lock, which is retryable.
#[test]
fn a_refused_lock_wait_fails_the_start_with_the_refusal() {
    let error = start("lock-refused");
    assert!(matches!(error, StartupError::Connect(ConnectError::Hook { .. })), "{error:?}");
    let permanent = refusal(&error);
    let failed = found::<MigrationFailed>(permanent).unwrap_or_else(|| panic!("the hook did not fail with `MigrationFailed`: {error:?}"));
    assert!(!failed.waited_for_lock() && !failed.is_retryable(), "a refused lock wait is classified as a wait for the lock: {failed:?}");
}

/// The control: a lock wait that reaches `connect_within` is a wait for another replica, retryable.
#[test]
fn a_lock_wait_that_passes_its_bound_stays_retryable() {
    let error = start_with("lock-expires", advance_once_waiting("lock-expires", CONNECT_WITHIN));
    assert!(!is_permanent(&error), "a lock wait that reached its bound is permanent: {error}");
    let failed = stored(&error).downcast_ref::<MigrationFailed>().unwrap_or_else(|| panic!("the hook did not fail with `MigrationFailed`: {error:?}"));
    assert!(failed.waited_for_lock(), "a lock wait that reached its bound is not a wait for the lock: {failed:?}");
    assert!(failed.source.is::<LockTimedOut>(), "{failed:?}");
}

/// Planted break: the migrations timed on `sleep`, so the refusal reads as `migrate_within`
/// passing.
#[test]
fn a_refused_migration_bound_fails_the_start_with_the_refusal() {
    scenario("migrate-refused").0.refuse(MIGRATE_WITHIN);
    let error = start("migrate-refused");
    let failed = found::<MigrationFailed>(refusal(&error)).unwrap_or_else(|| panic!("the hook did not fail with `MigrationFailed`: {error:?}"));
    assert!(failed.source.is::<RuntimeShutDown>(), "{failed:?}");
}

/// The control: migrations that run past `migrate_within` stay a permanent failure that is not the
/// refusal.
#[test]
fn migrations_that_pass_their_bound_stay_permanent() {
    let error = start_with("migrate-expires", advance_once_waiting("migrate-expires", MIGRATE_WITHIN));
    assert!(is_permanent(&error), "migrations past their bound are retryable: {error}");
    let permanent: &(dyn Error + 'static) =
        stored(&error).downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the start's error is not `Permanent`: {error:?}"));
    assert!(found::<RuntimeShutDown>(permanent).is_none(), "migrations past their bound report a refusal: {error:?}");
    let failed = found::<MigrationFailed>(permanent).unwrap_or_else(|| panic!("the hook did not fail with `MigrationFailed`: {error:?}"));
    assert!(!failed.is_retryable(), "{failed:?}");
}

/// A clock refusing every bound the migrations run under, `migrate_within` and the init hook's
/// own, fails the start with the refusal, `Permanent`, under the module's `OnModuleInit` hook.
#[test]
fn a_clock_refusing_the_migrations_and_the_hook_bound_fails_the_start_with_the_refusal() {
    let clock = scenario("migrate-all-refused").0;
    clock.refuse(MIGRATE_WITHIN);
    clock.refuse(HOOK_BOUND);
    let error = start("migrate-all-refused");
    let StartupError::Connect(ConnectError::Hook { hook, key, .. }) = &error else {
        panic!("the start failed other than in a hook: {error:?}");
    };
    assert_eq!(*hook, ulo::HookKind::OnModuleInit);
    // The store module is named by its own type, whatever its options.
    assert_eq!(key.to_string(), "StoreModule<Fake>", "the hook failure does not name the store's module");
    refusal(&error);
}

/// A connect bound the runtime refuses once it has passed reports the expiry, retryable, at the
/// construction bound.
#[test]
fn a_connect_bound_refused_after_it_passed_times_the_create_loop_out() {
    late_from("create-late").store(true, Ordering::SeqCst);
    let error = start_late("create-late", CONNECT_WITHIN);
    assert!(!is_permanent(&error), "a connect bound that passed is permanent: {error}");
    let StartupError::Connect(ConnectError::Construct { reason: FailureReason::TimedOut { after, limit }, .. }) = &error else {
        panic!("the loop did not time out at the construction bound: {error:?}");
    };
    assert_eq!((*after, *limit), (CONNECT_WITHIN, Limit::Item));
}

/// A lock wait the runtime refuses once it has passed is a wait for another replica, retryable.
#[test]
fn a_lock_wait_refused_after_it_passed_stays_retryable() {
    let error = start_late("lock-late", CONNECT_WITHIN);
    assert!(!is_permanent(&error), "a lock wait that reached its bound is permanent: {error}");
    let failed = stored(&error).downcast_ref::<MigrationFailed>().unwrap_or_else(|| panic!("the hook did not fail with `MigrationFailed`: {error:?}"));
    assert!(failed.waited_for_lock(), "a lock wait that reached its bound is not a wait for the lock: {failed:?}");
    assert!(failed.source.is::<LockTimedOut>(), "{failed:?}");
}

/// A `migrate_within` the runtime refuses once it has passed reports the bound passing, a
/// permanent failure that is not the refusal.
#[test]
fn a_migration_bound_refused_after_it_passed_reports_the_expiry() {
    late_from("migrate-late").store(true, Ordering::SeqCst);
    let error = start_late("migrate-late", MIGRATE_WITHIN);
    assert!(is_permanent(&error), "migrations past their bound are retryable: {error}");
    let permanent: &(dyn Error + 'static) =
        stored(&error).downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the start's error is not `Permanent`: {error:?}"));
    assert!(found::<RuntimeShutDown>(permanent).is_none(), "migrations past their bound report a refusal: {error:?}");
    let failed = found::<MigrationFailed>(permanent).unwrap_or_else(|| panic!("the hook did not fail with `MigrationFailed`: {error:?}"));
    assert!(!failed.is_retryable(), "{failed:?}");
}

