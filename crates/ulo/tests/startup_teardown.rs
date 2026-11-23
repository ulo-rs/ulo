//! The teardown of a connect walk that fails (section 9.2): the `OnModuleDestroy` hooks of every
//! binding and module that had initialized run in the exact reverse of their startup order,
//! each under its bound and all under `shutdown_timeout`, as at close, and what they record travels
//! in the `teardown` field beside the failure's `error`. A binding with no init hook has
//! initialized once it is built and checked, and a module with none once every singleton it
//! registers has initialized, wherever either sits relative to the failure; either also waits for
//! every singleton its destroy hooks read. A failed `load` tears down what it started the same
//! way.
//!
//! Timeouts run on `Frozen`, whose every sleep ends at once, and where the cap starts on `Stepped`,
//! whose clock the hooks move.

#[macro_use]
mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::*;
use ulo::{
    ConnectError, FailureReason, HookKind, Key, Limit, LoadError, ModuleDef, ShutdownFailure, StartupError,
};

struct A;
struct B;
struct C;
struct D;
struct L1;
struct L2;
struct S1;
struct S2;
struct S3;
struct S4;
struct Sc;
struct T1;
struct V;

/// An `on_init` closure logging `<name> init`, failing when `fail`.
fn init(log: &Log, name: &'static str, fail: bool) -> impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Boom>> + Send>> + Send + Sync + 'static {
    let log = log.clone();
    move || {
        let log = log.clone();
        Box::pin(async move {
            log.push(format!("{name} init"));
            if fail { Err(Boom::new(format!("{name} refused"))) } else { Ok(()) }
        })
    }
}

/// An `on_destroy` closure logging `<name> destroy`.
fn destroy(log: &Log, name: &'static str) -> impl Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync + 'static {
    let log = log.clone();
    move || {
        let log = log.clone();
        Box::pin(async move { log.push(format!("{name} destroy")) })
    }
}

/// An `on_destroy` closure reading `Dep<R>`, logging `<name> destroy`.
fn destroy_reading<R: Send + Sync + 'static>(log: &Log, name: &'static str) -> impl Fn(ulo::Dep<R>) -> std::future::Ready<()> + Send + Sync + 'static {
    let log = log.clone();
    move |_read: ulo::Dep<R>| {
        log.push(format!("{name} destroy"));
        std::future::ready(())
    }
}

fn destroyed(log: &Log) -> Vec<String> {
    log.events().into_iter().filter(|event| event.ends_with(" destroy")).collect()
}

/// S1 and S2 initialize, S3's init hook fails, S4's never runs; the module's own hooks follow its
/// singletons, so its init never ran either.
fn fails_at_s3(log: &Log) -> impl Fn(&mut ModuleDef<'_>) + Send + Sync + 'static {
    let log = log.clone();
    move |m| {
        m.singleton(|| async { S1 }).on_init(init(&log, "s1", false)).on_destroy(destroy(&log, "s1"));
        m.singleton(|| async { S2 }).on_init(init(&log, "s2", false)).on_destroy(destroy(&log, "s2"));
        m.singleton(|| async { S3 }).on_init(init(&log, "s3", true)).on_destroy(destroy(&log, "s3"));
        m.singleton(|| async { S4 }).on_init(init(&log, "s4", false)).on_destroy(destroy(&log, "s4"));
        m.on_init(init(&log, "module", false));
        m.on_destroy(destroy(&log, "module"));
    }
}

#[tokio::test]
async fn an_init_failure_runs_the_destroy_hooks_of_what_initialized_in_reverse_and_none_other() {
    let log = Log::new();
    let error = connect_err(Mod::<A>::new(fails_at_s3(&log))).await;
    let StartupError::Connect { error: connect_error, teardown, .. } = error else {
        panic!("expected the init hook's failure, got: {error}");
    };
    let ConnectError::Hook { hook, key, .. } = *connect_error else {
        panic!("expected the init hook's failure, got: {connect_error}");
    };
    assert_eq!((hook, key.key()), (HookKind::OnModuleInit, Key::of::<S3, ()>()));
    assert!(teardown.is_empty(), "every destroy hook ran, got: {teardown:?}");
    assert_eq!(log.events(), ["s1 init", "s2 init", "s3 init", "s2 destroy", "s1 destroy"]);
}

#[tokio::test]
async fn a_bootstrap_failure_runs_every_destroy_hook_in_reverse() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.provide::<BootFails>();
        m.singleton(|| async { S2 }).on_destroy(destroy(&l, "s2"));
        m.on_destroy(destroy(&l, "module"));
    }))
    .await;
    assert!(
        matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnApplicationBootstrap, .. })),
        "got: {error}"
    );
    assert_eq!(destroyed(&log), ["module destroy", "s2 destroy", "s1 destroy"], "every init stage completed");
}

#[tokio::test]
async fn a_construction_failure_runs_no_destroy_hook_of_an_owner_whose_init_hook_never_ran() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) });
        m.on_destroy(destroy(&l, "module"));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), Vec::<String>::new(), "no init hook ran, and the module's destroy hook waits for its singletons");
}

/// A destroy-only binding before the failing init hook and one after it are both torn down, in
/// the reverse of their startup order, interleaved with the owners whose init completed; the
/// module's destroy-only hook follows its singletons, which the init stage never passed.
#[tokio::test]
async fn an_init_failure_tears_down_a_destroy_only_binding_on_either_side_of_it_in_reverse() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_init(init(&l, "s2", false)).on_destroy(destroy(&l, "s2"));
        m.singleton(|| async { S3 }).on_init(init(&l, "s3", true)).on_destroy(destroy(&l, "s3"));
        m.singleton(|| async { S4 }).on_destroy(destroy(&l, "s4"));
        m.singleton(|| async { L1 }).on_init(init(&l, "l1", false)).on_destroy(destroy(&l, "l1"));
        m.on_destroy(destroy(&l, "module"));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s2 init", "s3 init", "s4 destroy", "s2 destroy", "s1 destroy"]);
}

/// The destroy-only bindings built and checked before a later construction fails are torn down in
/// reverse; one with an init hook is not, its init never having run.
#[tokio::test]
async fn a_construction_failure_tears_down_the_destroy_only_bindings_built_before_it_in_reverse() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_init(init(&l, "s2", false)).on_destroy(destroy(&l, "s2"));
        m.singleton(|| async { S3 }).on_destroy(destroy(&l, "s3")).ready(|_s: ulo::Dep<S3>| async { Ok::<(), Boom>(()) });
        m.try_singleton(|| async { Err::<S4, _>(Boom::new("down")) }).on_destroy(destroy(&l, "s4"));
        m.singleton(|| async { L1 }).on_destroy(destroy(&l, "l1"));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), ["s3 destroy", "s1 destroy"]);
}

/// A binding whose readiness check fails was built and not checked, so its destroy hook does not
/// run; the destroy-only binding checked before it is torn down.
#[tokio::test]
async fn a_readiness_failure_tears_down_the_destroy_only_bindings_checked_before_it_and_not_its_own() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_destroy(destroy(&l, "s2")).ready(|_s: ulo::Dep<S2>| async { Err::<(), _>(Boom::new("down")) });
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Readiness { .. })), "got: {error}");
    assert_eq!(log.events(), ["s1 destroy"]);
}

/// A binding after a failing readiness check was never built and runs no hook.
#[tokio::test]
async fn a_readiness_failure_runs_no_hook_of_a_binding_never_built() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).ready(|_s: ulo::Dep<S2>| async { Err::<(), _>(Boom::new("down")) });
        m.singleton(|| async { S3 }).on_destroy(destroy(&l, "s3"));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Readiness { .. })), "got: {error}");
    assert_eq!(log.events(), ["s1 destroy"]);
}

/// `B` has no init hook and its one singleton none either. `C`, collected first, holds the init
/// hook that fails, so `B` sits after the failure in startup order.
fn destroy_only_module_after(log: &Log, b_singleton_inits: bool) -> impl Fn(&mut ModuleDef<'_>) + Send + Sync + 'static {
    let log = log.clone();
    move |m| {
        let lc = log.clone();
        m.import(Mod::<C>::new(move |m| {
            m.singleton(|| async { S2 }).on_init(init(&lc, "s2", true)).on_destroy(destroy(&lc, "s2"));
        }));
        let lb = log.clone();
        m.import(Mod::<B>::new(move |m| {
            let s1 = m.singleton(|| async { S1 }).on_destroy(destroy(&lb, "s1"));
            if b_singleton_inits {
                s1.on_init(init(&lb, "s1", false));
            }
            m.on_destroy(destroy(&lb, "module-b"));
        }));
    }
}

/// A module with no init hook whose singletons all initialized is torn down, after the failure as
/// before it, in reverse startup order: its own hook before its singleton's.
#[tokio::test]
async fn an_init_failure_tears_down_a_destroy_only_module_after_it_whose_singletons_initialized() {
    let log = Log::new();
    let error = connect_err(Mod::<A>::new(destroy_only_module_after(&log, false))).await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s2 init", "module-b destroy", "s1 destroy"]);
}

/// A module with no init hook one of whose singletons has an init hook the stage never reached has
/// not initialized, and neither has that singleton.
#[tokio::test]
async fn an_init_failure_leaves_a_destroy_only_module_whose_singleton_never_initialized() {
    let log = Log::new();
    let error = connect_err(Mod::<A>::new(destroy_only_module_after(&log, true))).await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s2 init"]);
}

/// A module with an init hook the stage never reached has not initialized, whatever its
/// singletons did: its destroy hook does not run, its destroy-only singleton's does.
#[tokio::test]
async fn an_init_failure_leaves_a_later_module_whose_own_init_hook_never_ran() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let lc = l.clone();
        m.import(Mod::<C>::new(move |m| {
            m.singleton(|| async { S2 }).on_init(init(&lc, "s2", true));
        }));
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.singleton(|| async { S1 }).on_destroy(destroy(&lb, "s1"));
            m.on_init(init(&lb, "module-b", false));
            m.on_destroy(destroy(&lb, "module-b"));
        }));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s2 init", "s1 destroy"]);
}

/// A destroy-only module whose singletons all initialized before another module's init hook fails
/// is torn down after its providers in reverse; the failing module's destroy-only hook is not,
/// one of its singletons having failed.
#[tokio::test]
async fn an_init_failure_in_a_later_module_tears_down_a_destroy_only_module_before_it() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.singleton(|| async { S1 }).on_destroy(destroy(&lb, "s1"));
            m.on_destroy(destroy(&lb, "module-b"));
        }));
        m.singleton(|| async { S2 }).on_init(init(&l, "s2", true)).on_destroy(destroy(&l, "s2"));
        m.singleton(|| async { S3 }).on_init(init(&l, "s3", false)).on_destroy(destroy(&l, "s3"));
        m.singleton(|| async { S4 }).on_destroy(destroy(&l, "s4"));
        m.on_destroy(destroy(&l, "module-a"));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s2 init", "s4 destroy", "module-b destroy", "s1 destroy"]);
}

/// A construction failure in a later module tears down an earlier destroy-only module and its
/// destroy-only singleton, which have initialized, and a module registering no singleton; the
/// failing module has not.
#[tokio::test]
async fn a_construction_failure_in_a_later_module_tears_down_the_earlier_destroy_only_module_and_its_singleton() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.singleton(|| async { S1 }).on_destroy(destroy(&lb, "s1"));
            m.on_destroy(destroy(&lb, "module-b"));
        }));
        let lc = l.clone();
        m.import(Mod::<C>::new(move |m| {
            m.on_destroy(destroy(&lc, "module-c"));
        }));
        m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) }).on_destroy(destroy(&l, "s2"));
        m.singleton(|| async { S3 }).on_destroy(destroy(&l, "s3"));
        m.on_destroy(destroy(&l, "module-a"));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), ["module-c destroy", "module-b destroy", "s1 destroy"], "`C` registers no singleton");
}

#[tokio::test]
async fn a_destroy_hook_that_fails_is_reported_beside_the_failure_and_the_rest_still_run() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_init(init(&l, "s2", false)).on_destroy(|| async {
            if true {
                panic!("the destroy hook panics");
            }
        });
        m.singleton(|| async { S3 }).on_init(init(&l, "s3", false)).on_destroy(destroy(&l, "s3"));
        m.singleton(|| async { S4 }).on_init(init(&l, "s4", true));
    }))
    .await;
    let StartupError::Connect { error: connect_error, teardown, .. } = &error else {
        panic!("the failure is the init hook's, got: {error}");
    };
    let ConnectError::Hook { hook, key, .. } = &**connect_error else {
        panic!("the failure is the init hook's, got: {error}");
    };
    assert_eq!((*hook, key.key()), (HookKind::OnModuleInit, Key::of::<S4, ()>()));
    let [ShutdownFailure::Hook { hook, key, reason }] = teardown.as_slice() else {
        panic!("one destroy hook failed, got: {teardown:?}");
    };
    assert_eq!((*hook, key.key()), (HookKind::OnModuleDestroy, Key::of::<S2, ()>()));
    assert!(matches!(**reason, FailureReason::Panicked(_)), "got: {reason}");
    assert_eq!(destroyed(&log), ["s3 destroy", "s1 destroy"], "the hooks on either side of the panicking one ran");
    let text = error.to_string();
    assert!(text.starts_with("`OnModuleInit` hook of `S4` failed"), "the failure reads first: {text}");
    assert!(text.contains("the destroy hooks run for what had started recorded 1 failure"), "{text}");
    assert!(text.contains("`OnModuleDestroy` hook of `S2`: panicked"), "{text}");
    let boxed: ulo::BoxError = Box::new(error);
    assert!(!ulo::is_panic(&boxed), "the panic is the teardown's, and the failure an init hook's `Err`");
}

#[tokio::test]
async fn a_failed_load_reports_its_teardown_beside_the_failure() {
    let app = connects(Empty).await;
    let handle = app.handle();
    let error = handle
        .load(Mod::<L1>::new(|m| {
            m.singleton(|| async { S1 }).on_destroy(|| async {
                if true {
                    panic!("the destroy hook panics");
                }
            });
            m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) });
        }))
        .await
        .err()
        .expect("the load succeeded");
    let LoadError::Connect { error: connect_error, teardown, .. } = &error else {
        panic!("expected a construction failure, got: {error}");
    };
    let ConnectError::Construct { key, .. } = &**connect_error else {
        panic!("expected a construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<S2, ()>());
    let [ShutdownFailure::Hook { key, reason, .. }] = teardown.as_slice() else {
        panic!("S1's destroy hook panicked, got: {teardown:?}");
    };
    assert!(matches!(**reason, FailureReason::Panicked(_)), "S1's destroy hook panicked, got: {teardown:?}");
    assert_eq!(key.key(), Key::of::<S1, ()>());
    let text = error.to_string();
    assert!(text.starts_with("failed to construct `S2`"), "the failure reads first: {text}");
    assert!(text.contains("recorded 1 failure\n  × `OnModuleDestroy` hook of `S1`: panicked"), "{text}");
    close(app).await;
}

#[tokio::test]
async fn a_destroy_hook_past_its_bound_is_cut_at_the_bound() {
    watchdog();
    let log = Log::new();
    let l = log.clone();
    let app = ulo::App::builder(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 })
            .on_init(init(&l, "s2", false))
            .on_destroy(std::future::pending::<()>)
            .timeout(Duration::from_secs(3));
        m.singleton(|| async { S3 }).on_init(init(&l, "s3", true));
    }))
    .timer(Frozen::new())
    .wire()
    .unwrap();
    let error = within("connect", app.connect()).await.err().expect("connect succeeded with a failing init hook");
    let StartupError::Connect { teardown, .. } = &error else { panic!("expected a connect failure, got: {error}") };
    let [ShutdownFailure::Hook { key, reason, .. }] = teardown.as_slice() else {
        panic!("the destroy hook timed out, got: {teardown:?}");
    };
    let FailureReason::TimedOut { after, limit } = &**reason else {
        panic!("the destroy hook timed out, got: {teardown:?}");
    };
    assert_eq!(key.key(), Key::of::<S2, ()>());
    assert_eq!((*after, *limit), (Duration::from_secs(3), Limit::Item));
    assert_eq!(destroyed(&log), ["s1 destroy"], "the hook after the cut one ran");
}

#[tokio::test]
async fn an_expired_shutdown_timeout_skips_the_destroy_hooks_not_yet_run() {
    watchdog();
    let log = Log::new();
    let l = log.clone();
    let app = ulo::App::builder(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_init(init(&l, "s2", true));
    }))
    .timer(Frozen::new())
    .shutdown_timeout(Duration::ZERO)
    .wire()
    .unwrap();
    let error = within("connect", app.connect()).await.err().expect("connect succeeded with a failing init hook");
    let StartupError::Connect { teardown, .. } = &error else { panic!("expected a connect failure, got: {error}") };
    let [ShutdownFailure::Hook { key, reason, .. }] = teardown.as_slice() else {
        panic!("the destroy hook was skipped, got: {teardown:?}");
    };
    assert!(matches!(**reason, FailureReason::Skipped), "the destroy hook was skipped, got: {teardown:?}");
    assert_eq!(key.key(), Key::of::<S1, ()>());
    assert_eq!(destroyed(&log), Vec::<String>::new());
}

#[tokio::test]
async fn a_load_whose_construction_fails_tears_down_the_destroy_only_bindings_it_built_once() {
    let log = Log::new();
    let app = connects(Empty).await;
    let handle = app.handle();
    let l = log.clone();
    let error = handle
        .load(Mod::<L1>::new(move |m| {
            m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
            m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) });
        }))
        .await
        .err()
        .expect("the load succeeded");
    assert!(matches!(&error, LoadError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    close(app).await;
    assert_eq!(destroyed(&log), ["s1 destroy"], "the app's own close does not run it again");
}

/// A clock that moves only when a hook moves it, and whose sleeps never end, so every hook here
/// finishes on its first poll and no bound fires on its own.
#[derive(Clone)]
struct Stepped(Arc<Mutex<Instant>>);

impl Stepped {
    fn new() -> Self {
        Stepped(Arc::new(Mutex::new(Instant::now() + Duration::from_secs(86_400))))
    }

    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl ulo::Timer for Stepped {
    fn sleep(&self, _d: Duration) -> ulo::BoxFuture<'static, ()> {
        Box::pin(std::future::pending())
    }

    fn now(&self) -> Instant {
        *self.0.lock().unwrap()
    }

    fn try_sleep(&self, _d: Duration) -> ulo::BoxFuture<'static, Result<(), ulo::Refused>> {
        Box::pin(std::future::pending())
    }
}

/// The `shutdown_timeout` cap starts as the teardown begins: the failing init hook moves the clock
/// past the cap's length, measured from the start of `connect`, and S2's destroy hook still runs;
/// S2's hook then moves it past the cap measured from the teardown's start, and S1's, the next,
/// is skipped.
#[tokio::test]
async fn the_teardown_cap_starts_when_the_teardown_begins() {
    watchdog();
    let log = Log::new();
    let clock = Stepped::new();
    let (l, c) = (log.clone(), clock.clone());
    let app = ulo::App::builder(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
        let (l2, c2) = (l.clone(), c.clone());
        m.singleton(|| async { S2 }).on_destroy(move || {
            l2.push("s2 destroy");
            c2.advance(Duration::from_secs(6));
            std::future::ready(())
        });
        let c3 = c.clone();
        m.singleton(|| async { S3 }).on_init(move || {
            c3.advance(Duration::from_secs(10));
            std::future::ready(Err::<(), _>(Boom::new("s3 refused")))
        });
    }))
    .timer(clock)
    .shutdown_timeout(Duration::from_secs(5))
    .wire()
    .unwrap();
    let error = within("connect", app.connect()).await.err().expect("connect succeeded with a failing init hook");
    let StartupError::Connect { teardown, .. } = &error else { panic!("expected a connect failure, got: {error}") };
    let [ShutdownFailure::Hook { key, reason, .. }] = teardown.as_slice() else {
        panic!("S1's destroy hook was skipped and no other failure recorded, got: {teardown:?}");
    };
    assert!(matches!(**reason, FailureReason::Skipped), "S1's destroy hook was skipped and no other failure recorded, got: {teardown:?}");
    assert_eq!(key.key(), Key::of::<S1, ()>());
    assert_eq!(destroyed(&log), ["s2 destroy"]);
}

#[tokio::test]
async fn a_failed_load_runs_the_destroy_hooks_of_what_it_initialized() {
    let log = Log::new();
    let app = connects(Empty).await;
    let handle = app.handle();
    let error = handle.load(Mod::<L1>::new(fails_at_s3(&log))).await.err().expect("the load succeeded");
    assert!(matches!(&error, LoadError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s1 init", "s2 init", "s3 init", "s2 destroy", "s1 destroy"]);
    close(app).await;
    assert_eq!(destroyed(&log), ["s2 destroy", "s1 destroy"], "the app's own close does not run them again");
}

/// A module whose own init hook fails after its singletons initialized has not initialized; its
/// singletons have, and so has a module after it registering no singleton.
#[tokio::test]
async fn a_module_whose_own_init_hook_fails_is_not_torn_down_and_its_singletons_are() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.singleton(|| async { S1 }).on_init(init(&lb, "s1", false)).on_destroy(destroy(&lb, "s1"));
            m.singleton(|| async { S2 }).on_destroy(destroy(&lb, "s2"));
            m.on_init(init(&lb, "module-b", true));
            m.on_destroy(destroy(&lb, "module-b"));
        }));
        let ld = l.clone();
        m.import(Mod::<D>::new(move |m| {
            m.on_destroy(destroy(&ld, "module-d"));
        }));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s1 init", "module-b init", "module-d destroy", "s2 destroy", "s1 destroy"]);
}

/// A failed load leaves the base graph as it was: the same module loads again, and the app's close
/// runs the hooks of that second load once, not those the failed one ran.
#[tokio::test]
async fn a_module_whose_load_failed_loads_again_and_closes_once() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let log = Log::new();
    let app = connects(Empty).await;
    let handle = app.handle();
    let fails = Arc::new(AtomicBool::new(true));
    let lazy = |log: &Log, fails: &Arc<AtomicBool>| {
        let (l, f) = (log.clone(), fails.clone());
        Mod::<L1>::new(move |m| {
            let ld = l.clone();
            m.import(Mod::<L2>::new(move |m| {
                m.on_destroy(destroy(&ld, "module-l2"));
            }));
            m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
            let fails = f.load(Ordering::SeqCst);
            m.try_singleton(move || async move { if fails { Err(Boom::new("down")) } else { Ok(S2) } });
        })
    };
    let error = handle.load(lazy(&log, &fails)).await.err().expect("the load succeeded");
    assert!(matches!(&error, LoadError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(destroyed(&log), ["s1 destroy", "module-l2 destroy"]);
    fails.store(false, Ordering::SeqCst);
    handle.load(lazy(&log, &fails)).await.expect("the second load connects");
    close(app).await;
    assert_eq!(destroyed(&log), ["s1 destroy", "module-l2 destroy", "s1 destroy", "module-l2 destroy"]);
}

/// A singleton whose bootstrap hook fails, after every init hook has run.
struct BootFails;

impl ulo::Construct for BootFails {
    type Scope = ulo::scope::Singleton;

    fn hooks(h: &mut ulo::Hooks<Self>) {
        h.on_application_bootstrap();
    }

    fn dependencies(_d: &mut ulo::Dependencies) {}

    async fn construct(_r: &ulo::Resolver<'_>) -> Result<Self, ulo::ConstructError> {
        Ok(BootFails)
    }
}

impl ulo::OnApplicationBootstrap for BootFails {
    async fn on_application_bootstrap(&self) -> Result<(), ulo::BoxError> {
        Err(Box::new(Boom::new("bootstrap refused")))
    }
}

/// A `dyn TraceSource` bound per execution, which `connect` cannot read without one.
struct PerCallSource;

impl ulo::TraceSource for PerCallSource {
    fn start(&self, _span: &tracing::Span, _inbound: Option<&ulo::TraceContext>) -> ulo::TraceContext {
        unreachable!("never read: `connect` refuses a source bound per execution")
    }
}

#[tokio::test]
async fn a_trace_source_failure_runs_every_destroy_hook_in_reverse() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_destroy(destroy(&l, "s2"));
        m.execution(|| async { PerCallSource }).also_as::<dyn ulo::TraceSource>(|a| a);
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(destroyed(&log), ["s2 destroy", "s1 destroy"], "the source is read once every init stage completed");
}

/// `C` holds `Sc`, whose construction fails. `B` registers no singleton and has no init hook, and
/// its destroy hook reads `Sc`: `B` has not initialized, so the hook does not run and the teardown
/// records nothing, leaving the construction failure alone.
#[tokio::test]
async fn a_destroy_only_module_whose_hook_reads_a_singleton_never_built_is_not_torn_down() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.import(Mod::<C>::new(|m| {
                m.try_singleton(|| async { Err::<Sc, _>(Boom::new("down")) });
                m.export::<Sc>();
            }));
            m.on_destroy(destroy_reading::<Sc>(&lb, "module-b"));
        }));
    }))
    .await;
    let StartupError::Connect { error: connect_error, teardown, .. } = &error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    let ConnectError::Construct { key, .. } = &**connect_error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<Sc, ()>());
    assert!(teardown.is_empty(), "no hook ran whose dependency was never built, got: {teardown:?}");
    assert_eq!(log.events(), Vec::<String>::new());
}

/// A destroy-only binding built before a later construction fails, whose destroy hook reads the
/// binding that failed, has not initialized; one beside it reading nothing is torn down.
#[tokio::test]
async fn a_destroy_only_binding_whose_hook_reads_a_singleton_never_built_is_not_torn_down() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy(&l, "s1"));
        m.singleton(|| async { S2 }).on_destroy(destroy_reading::<Sc>(&l, "s2"));
        m.try_singleton(|| async { Err::<Sc, _>(Boom::new("down")) });
    }))
    .await;
    let StartupError::Connect { error: connect_error, teardown, .. } = &error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    let ConnectError::Construct { key, .. } = &**connect_error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<Sc, ()>());
    assert!(teardown.is_empty(), "no hook ran whose dependency was never built, got: {teardown:?}");
    assert_eq!(log.events(), ["s1 destroy"]);
}

/// A destroy hook reading a transient waits for the singletons the transient reads.
#[tokio::test]
async fn a_destroy_hook_reading_a_transient_waits_for_the_singletons_it_reads() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.transient(|_sc: ulo::Dep<Sc>| async { T1 });
        m.singleton(|| async { S1 }).on_destroy(destroy_reading::<T1>(&l, "s1"));
        m.try_singleton(|| async { Err::<Sc, _>(Boom::new("down")) });
    }))
    .await;
    let StartupError::Connect { error: connect_error, teardown, .. } = &error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    let ConnectError::Construct { key, .. } = &**connect_error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<Sc, ()>());
    assert!(teardown.is_empty(), "no hook ran whose dependency was never built, got: {teardown:?}");
    assert_eq!(log.events(), Vec::<String>::new());
}

/// A destroy-only module whose hook reads a singleton checked before a later construction fails
/// has initialized, and its hook runs.
#[tokio::test]
async fn a_destroy_only_module_whose_hook_reads_a_singleton_checked_before_the_failure_is_torn_down() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.import(Mod::<C>::new(|m| {
                m.singleton(|| async { S1 });
                m.export::<S1>();
            }));
            m.on_destroy(destroy_reading::<S1>(&lb, "module-b"));
        }));
        m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) });
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), ["module-b destroy"]);
}

/// A value with no readiness check exists from `wire()`: a destroy-only binding built before a
/// failing construction, whose hook reads a value declared after it, has initialized.
#[tokio::test]
async fn a_destroy_only_binding_whose_hook_reads_a_value_the_walk_had_not_reached_is_torn_down() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_destroy(destroy_reading::<V>(&l, "s1"));
        m.try_singleton(|| async { Err::<Sc, _>(Boom::new("down")) });
        m.value(V);
    }))
    .await;
    let StartupError::Connect { error: connect_error, teardown, .. } = &error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    let ConnectError::Construct { key, .. } = &**connect_error else {
        panic!("expected `Sc`'s construction failure, got: {error}");
    };
    assert_eq!(key.key(), Key::of::<Sc, ()>());
    assert!(teardown.is_empty(), "the hook read `V`, got: {teardown:?}");
    assert_eq!(log.events(), ["s1 destroy"]);
}

/// A module registering no singleton whose destroy hook reads a value of a module collected after
/// the failing construction has initialized.
#[tokio::test]
async fn a_destroy_only_module_whose_hook_reads_a_value_the_walk_had_not_reached_is_torn_down() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.import(Mod::<C>::new(|m| {
            m.try_singleton(|| async { Err::<Sc, _>(Boom::new("down")) });
        }));
        m.import(Mod::<D>::new(|m| {
            m.value(V);
            m.export::<V>();
        }));
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.import(Mod::<D>::bare());
            m.on_destroy(destroy_reading::<V>(&lb, "module-b"));
        }));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), ["module-b destroy"]);
}

/// A singleton whose readiness check failed is built and not checked: a destroy-only module whose
/// hook reads it has not initialized, as the singleton's own destroy hook does not run.
#[tokio::test]
async fn a_destroy_hook_reading_a_singleton_whose_check_failed_does_not_run() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.import(Mod::<C>::new(|m| {
            m.singleton(|| async { S1 }).ready(|_s: ulo::Dep<S1>| async { Err::<(), _>(Boom::new("down")) });
            m.export::<S1>();
        }));
        let lb = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.import(Mod::<C>::bare());
            m.on_destroy(destroy_reading::<S1>(&lb, "module-b"));
        }));
    }))
    .await;
    assert!(matches!(&error, StartupError::Connect { error, .. } if matches!(**error, ConnectError::Readiness { .. })), "got: {error}");
    assert_eq!(log.events(), Vec::<String>::new());
}

/// A lazily loaded module's destroy hook reading a singleton of the graph it extends, which
/// connected before the load, runs when the load fails.
#[tokio::test]
async fn a_failed_load_tears_down_a_module_whose_hook_reads_the_base_graph() {
    let log = Log::new();
    let app = connects(Mod::<A>::new(|m| {
        m.import(Mod::<C>::new(|m| {
            m.singleton(|| async { S1 });
            m.export::<S1>();
        }));
    }))
    .await;
    let handle = app.handle();
    let l = log.clone();
    let error = handle
        .load(Mod::<L1>::new(move |m| {
            m.import(Mod::<C>::bare());
            m.import(Mod::<L2>::new(|m| {
                m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) });
            }));
            m.on_destroy(destroy_reading::<S1>(&l, "module-l1"));
        }))
        .await
        .err()
        .expect("the load succeeded");
    assert!(matches!(&error, LoadError::Connect { error, .. } if matches!(**error, ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), ["module-l1 destroy"]);
    close(app).await;
}

/// A destroy hook whose parameter read fails on a `Permanent` is recorded in the teardown, and the
/// start is still retryable: `is_permanent` reads the walk's own failure.
#[tokio::test]
async fn a_permanent_failure_in_the_teardown_leaves_the_start_retryable() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.try_transient(|| async { Err::<T1, _>(ulo::Permanent::new(Boom::new("bad credentials"))) });
        m.singleton(|| async { S1 }).on_destroy(destroy_reading::<T1>(&l, "s1"));
        m.singleton(|| async { S2 }).on_init(init(&l, "s2", true));
    }))
    .await;
    let StartupError::Connect { teardown, .. } = &error else { panic!("expected a connect failure, got: {error}") };
    let [ShutdownFailure::Hook { key, reason, .. }] = teardown.as_slice() else {
        panic!("S1's destroy hook failed its read, got: {teardown:?}");
    };
    assert!(matches!(**reason, FailureReason::Errored(_)), "S1's destroy hook failed its read, got: {teardown:?}");
    assert_eq!(key.key(), Key::of::<S1, ()>());
    assert!(!ulo::is_permanent(&error), "the init hook's failure carries no `Permanent`: {error}");
}
