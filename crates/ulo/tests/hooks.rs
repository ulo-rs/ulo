//! Lifecycle hooks (section 9.1, 9.2) at run time.
//!
//! Capabilities pinned: 39 (async hooks: module init then application bootstrap, either can fail
//! and fail startup; at close before-shutdown, destroy and shutdown, the signal name reaching the
//! two that take it; a module has hooks of its own), 40 (startup hooks run in dependency order and
//! shutdown in the exact reverse), and the connect rows of section 3 for hooks: a hook that
//! errs, panics or outlives its bound fails `connect` with `ConnectError::Hook` and the
//! `FailureReason` that says how.
//!
//! Trait hooks come from a hand-written `Construct` impl (`hooked!`); closure hooks from a binding
//! handle or the module. Only a trait can register `OnApplicationBootstrap`. Shutdown-side
//! failures (a hook that panics, times out or is skipped) are in `shutdown.rs`.
//!
//! Timeouts run on `Frozen`, whose every sleep ends at once, so an item that is still pending when
//! polled is abandoned in that same poll and nothing waits on a real clock.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::scope::Singleton;
use ulo::{
    BoxError, ConnectError, Construct, ConstructError, Dep, Dependencies, FailureReason, HookKind, Hooks, Key, Limit, LookupError,
    ModuleDef, OnApplicationBootstrap, OnApplicationShutdown, OnModuleDestroy, OnModuleInit, BeforeApplicationShutdown, Resolver,
    Signal, StartupError,
};

struct A;
struct B;
struct S1;
struct S2;
struct S3;

/// A singleton of its own type that registers all five hook traits, each logging
/// `<label> <hook>` (the two that take the signal add its name), and `<label> built` when built.
macro_rules! hooked {
    ($name:ident, $label:literal) => {
        struct $name(Log);

        impl Construct for $name {
            type Scope = Singleton;

            fn hooks(h: &mut Hooks<Self>) {
                h.on_module_init();
                h.on_application_bootstrap();
                h.on_module_destroy();
                h.before_application_shutdown();
                h.on_application_shutdown();
            }

            fn dependencies(d: &mut Dependencies) {
                d.field::<Dep<Log>>("log");
            }

            async fn construct(r: &Resolver<'_>) -> Result<Self, ConstructError> {
                let log = r.dep::<Log>().await?;
                log.push(concat!($label, " built"));
                Ok($name(log.arc().as_ref().clone()))
            }
        }

        impl OnModuleInit for $name {
            async fn on_module_init(&self) -> Result<(), BoxError> {
                self.0.push(concat!($label, " init"));
                Ok(())
            }
        }

        impl OnApplicationBootstrap for $name {
            async fn on_application_bootstrap(&self) -> Result<(), BoxError> {
                self.0.push(concat!($label, " bootstrap"));
                Ok(())
            }
        }

        impl OnModuleDestroy for $name {
            async fn on_module_destroy(&self) {
                self.0.push(concat!($label, " destroy"));
            }
        }

        impl BeforeApplicationShutdown for $name {
            async fn before_application_shutdown(&self, signal: &Signal) {
                self.0.push(format!(concat!($label, " before-shutdown {}"), signal.name()));
            }
        }

        impl OnApplicationShutdown for $name {
            async fn on_application_shutdown(&self, signal: &Signal) {
                self.0.push(format!(concat!($label, " on-shutdown {}"), signal.name()));
            }
        }
    };
}

hooked!(T1, "t1");
hooked!(T2, "t2");

/// A trait-hooked singleton whose bootstrap hook returns `Err`.
struct BootFails;

impl Construct for BootFails {
    type Scope = Singleton;

    fn hooks(h: &mut Hooks<Self>) {
        h.on_application_bootstrap();
    }

    fn dependencies(_d: &mut Dependencies) {}

    async fn construct(_r: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(BootFails)
    }
}

impl OnApplicationBootstrap for BootFails {
    async fn on_application_bootstrap(&self) -> Result<(), BoxError> {
        Err(Box::new(Boom::new("bootstrap refused")))
    }
}

/// The hook events of `log`, without the `built` ones.
fn hook_events(log: &Log) -> Vec<String> {
    log.events().into_iter().filter(|e| !e.ends_with(" built")).collect()
}


/// An `on_init` closure logging `$text`.
macro_rules! log_init {
    ($log:expr, $text:expr) => {{
        let log = $log.clone();
        move || {
            let log = log.clone();
            async move {
                log.push($text);
                Ok::<(), Boom>(())
            }
        }
    }};
}

/// An `on_destroy` closure logging `$text`.
macro_rules! log_destroy {
    ($log:expr, $text:expr) => {{
        let log = $log.clone();
        move || {
            let log = log.clone();
            async move { log.push($text) }
        }
    }};
}

/// A shutdown-hook closure logging `$text` and the signal's name, `<text> <signal>`; for
/// `before_shutdown` and `on_shutdown`.
macro_rules! log_signal {
    ($log:expr, $text:expr) => {{
        let log = $log.clone();
        move |signal: Signal| {
            let log = log.clone();
            async move { log.push(format!("{} {}", $text, signal.name())) }
        }
    }};
}

/// A module's four hooks, each logging `<name> module <hook>`.
fn module_hooks(m: &mut ModuleDef<'_>, name: &'static str, log: &Log) {
    m.on_init(log_init!(log, format!("{name} module init")));
    m.on_destroy(log_destroy!(log, format!("{name} module destroy")));
    let (before, after) = (log.clone(), log.clone());
    m.before_shutdown(move |_s: Signal| {
        let log = before.clone();
        async move { log.push(format!("{name} module before-shutdown")) }
    });
    m.on_shutdown(move |_s: Signal| {
        let log = after.clone();
        async move { log.push(format!("{name} module on-shutdown")) }
    });
}

#[tokio::test]
async fn init_hooks_all_run_before_any_bootstrap_hook_in_connect_order() {
    let log = Log::new();
    let l = log.clone();
    let app = connects(Mod::<A>::new(move |m| {
        m.value(l.clone());
        m.provide::<T1>();
        m.provide::<T2>();
        let built = l.clone();
        m.singleton(move || {
            built.push("c built");
            async { S1 }
        })
        .on_init(log_init!(l, "c init"));
    }))
    .await;
    assert_eq!(
        log.events(),
        ["t1 built", "t2 built", "c built", "t1 init", "t2 init", "c init", "t1 bootstrap", "t2 bootstrap"],
        "every singleton is built, then every init hook runs in connect order, then every bootstrap hook"
    );
    close(app).await;
}

#[tokio::test]
async fn a_failing_init_hook_fails_connect_naming_the_hook_and_the_key_and_stops_the_walk() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.value(l.clone());
        let hook = |name: &'static str, fail: bool| {
            let log = l.clone();
            move || {
                let log = log.clone();
                async move {
                    log.push(format!("{name} init"));
                    if fail { Err(Boom::new("init refused")) } else { Ok(()) }
                }
            }
        };
        m.singleton(|| async { S1 }).on_init(hook("s1", false));
        m.singleton(|| async { S2 }).on_init(hook("s2", true));
        m.singleton(|| async { S3 }).on_init(hook("s3", false));
        m.provide::<T1>();
    }))
    .await;

    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!(hook, HookKind::OnModuleInit);
    assert_eq!(key.key(), Key::of::<S2, ()>());
    let FailureReason::Errored(source) = reason else { panic!("an `Err` is `Errored`, got: {reason}") };
    assert!(source.downcast_ref::<Boom>().is_some(), "the hook's own error is reachable by its type");
    assert_eq!(
        hook_events(&log),
        ["s1 init", "s2 init"],
        "the walk stops at the failure: no later init hook, and no bootstrap hook, ran"
    );
}

#[tokio::test]
async fn a_failing_bootstrap_hook_fails_connect_after_every_init_hook_ran() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.value(l.clone());
        m.provide::<T1>();
        m.provide::<BootFails>();
        m.provide::<T2>();
    }))
    .await;

    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!(hook, HookKind::OnApplicationBootstrap);
    assert_eq!(key.key(), Key::of::<BootFails, ()>());
    assert!(matches!(reason, FailureReason::Errored(_)), "got: {reason}");
    assert_eq!(
        hook_events(&log),
        ["t1 init", "t2 init", "t1 bootstrap"],
        "init hooks all ran; bootstrap stopped at the failing one, so the next bootstrap hook did not"
    );
}

#[tokio::test]
async fn a_panicking_hook_is_a_panicked_failure_the_core_recognises() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 }).on_init(|| async {
            if true {
                panic!("the hook panics");
            }
            Ok::<(), Boom>(())
        });
    }))
    .await;

    let boxed: BoxError = Box::new(error);
    assert!(ulo::is_panic(&boxed), "`is_panic` finds the panic under the startup error");
    let error = boxed.downcast::<StartupError>().unwrap();
    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = *error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!(hook, HookKind::OnModuleInit);
    assert_eq!(key.key(), Key::of::<S1, ()>());
    assert!(matches!(reason, FailureReason::Panicked(_)), "got: {reason}");
}

#[tokio::test]
async fn a_hook_that_outlives_its_own_bound_is_timed_out_with_the_item_limit() {
    watchdog();
    let app = ulo::App::builder(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 })
            .on_init(|| async {
                std::future::pending::<()>().await;
                Ok::<(), Boom>(())
            })
            .timeout(Duration::from_secs(3));
    }))
    .timer(Frozen::new())
    .wire()
    .unwrap();
    let error = within("connect", app.connect()).await.err().expect("connect succeeded with a hook that never returns");

    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!((hook, key.key()), (HookKind::OnModuleInit, Key::of::<S1, ()>()));
    let FailureReason::TimedOut { after, limit } = reason else { panic!("expected a timeout, got: {reason}") };
    assert_eq!((after, limit), (Duration::from_secs(3), Limit::Item));
}

#[tokio::test]
async fn a_hook_with_no_bound_of_its_own_takes_the_apps_hook_timeout() {
    watchdog();
    let app = ulo::App::builder(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 }).on_init(|| async {
            std::future::pending::<()>().await;
            Ok::<(), Boom>(())
        });
    }))
    .timer(Frozen::new())
    .hook_timeout(Duration::from_secs(7))
    .wire()
    .unwrap();
    let error = within("connect", app.connect()).await.err().expect("connect succeeded with a hook that never returns");

    let StartupError::Connect(ConnectError::Hook { reason: FailureReason::TimedOut { after, limit }, .. }) = error else {
        panic!("expected a hook timeout, got: {error}");
    };
    assert_eq!((after, limit), (Duration::from_secs(7), Limit::Default));
}

#[tokio::test]
async fn close_runs_before_shutdown_then_destroy_then_shutdown_each_in_reverse_with_the_signal_name() {
    let log = Log::new();
    let l = log.clone();
    let app = connects(Mod::<A>::new(move |m| {
        m.value(l.clone());
        m.provide::<T1>();
        m.provide::<T2>();
        m.singleton(|| async { S1 })
            .before_shutdown(log_signal!(l, "c before-shutdown"))
            .on_destroy(log_destroy!(l, "c destroy"))
            .on_shutdown(log_signal!(l, "c on-shutdown"));
    }))
    .await;
    let startup = log.events().len();

    let report = app.close(Signal::new("done")).await.expect("shutdown reported a failure");
    assert_eq!(report.signal.name(), "done");
    assert_eq!(
        log.events()[startup..],
        [
            "c before-shutdown done",
            "t2 before-shutdown done",
            "t1 before-shutdown done",
            "c destroy",
            "t2 destroy",
            "t1 destroy",
            "c on-shutdown done",
            "t2 on-shutdown done",
            "t1 on-shutdown done",
        ],
        "before-shutdown, destroy, then on-shutdown, each step in reverse connect order, the signal's name reaching the two that take it"
    );
}

#[tokio::test]
async fn a_module_hook_runs_after_its_providers_hooks_and_before_the_next_module() {
    let log = Log::new();
    let l = log.clone();
    let app = connects(Mod::<A>::new(move |m| {
        let b_log = l.clone();
        m.import(Mod::<B>::new(move |m| {
            m.singleton(|| async { S2 }).on_init(log_init!(b_log, "b init")).on_destroy(log_destroy!(b_log, "b destroy"));
            module_hooks(m, "b", &b_log);
        }));
        m.singleton(|| async { S1 }).on_init(log_init!(l, "r init")).on_destroy(log_destroy!(l, "r destroy"));
        module_hooks(m, "root", &l);
    }))
    .await;
    assert_eq!(
        log.events(),
        ["b init", "b module init", "r init", "root module init"],
        "a module's own init hook follows its providers' and precedes the next module's"
    );
    let startup = log.events().len();

    app.close(Signal::new("done")).await.expect("shutdown reported a failure");
    assert_eq!(
        log.events()[startup..],
        [
            "root module before-shutdown",
            "b module before-shutdown",
            "root module destroy",
            "r destroy",
            "b module destroy",
            "b destroy",
            "root module on-shutdown",
            "b module on-shutdown",
        ],
        "shutdown is the exact reverse: the module's own hook first, then its providers'"
    );
}

#[tokio::test]
async fn a_failing_module_hook_names_the_module() {
    let error = connect_err(Mod::<A>::new(|m| {
        m.on_init(|| async { Err::<(), _>(Boom::new("module init refused")) });
    }))
    .await;

    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!(hook, HookKind::OnModuleInit);
    assert_eq!(key.key(), Key::of::<Mod<A>, ()>(), "a module hook is named by the module's own type");
    assert!(matches!(reason, FailureReason::Errored(_)), "got: {reason}");
}

/// A singleton built by `$factory`, with init and destroy closure hooks logging `$label`.
macro_rules! link {
    ($m:expr, $log:expr, $label:literal, $factory:expr) => {{
        $m.singleton($factory)
            .on_init(log_init!($log, concat!($label, " init")))
            .on_destroy(log_destroy!($log, concat!($label, " destroy")));
    }};
}

struct Ca;
struct Cb;
struct Cc;

#[tokio::test]
async fn hooks_run_in_dependency_order_and_shutdown_in_reverse_for_a_chain() {
    let log = Log::new();
    let l = log.clone();
    // Declared against the dependency order: `Ca` reads `Cb` reads `Cc`, and `Cc` is written last.
    let app = connects(Mod::<A>::new(move |m| {
        link!(m, l, "ca", |_b: Dep<Cb>| async { Ca });
        link!(m, l, "cb", |_c: Dep<Cc>| async { Cb });
        link!(m, l, "cc", || async { Cc });
    }))
    .await;
    assert_eq!(log.events(), ["cc init", "cb init", "ca init"], "a provider's hook runs after the hooks of what it depends on");
    app.close(Signal::new("done")).await.unwrap();
    assert_eq!(
        log.events()[3..],
        ["ca destroy", "cb destroy", "cc destroy"],
        "destroy hooks run in the exact reverse"
    );
}

struct Da;
struct Db;
struct Dc;
struct Dd;

#[tokio::test]
async fn hooks_in_a_diamond_follow_every_edge_and_shutdown_is_the_exact_reverse() {
    let log = Log::new();
    let l = log.clone();
    // `Dd` reads `Db` and `Dc`, each of which reads `Da`; declared with the dependent first.
    let app = connects(Mod::<A>::new(move |m| {
        link!(m, l, "dd", |_b: Dep<Db>, _c: Dep<Dc>| async { Dd });
        link!(m, l, "dc", |_a: Dep<Da>| async { Dc });
        link!(m, l, "db", |_a: Dep<Da>| async { Db });
        link!(m, l, "da", || async { Da });
    }))
    .await;
    let init: Vec<String> = log.events();
    let at = |name: &str| init.iter().position(|e| e == name).unwrap_or_else(|| panic!("`{name}` never ran: {init:?}"));
    assert!(at("da init") < at("db init") && at("da init") < at("dc init"), "{init:?}");
    assert!(at("db init") < at("dd init") && at("dc init") < at("dd init"), "{init:?}");
    assert_eq!(init.len(), 4);

    app.close(Signal::new("done")).await.unwrap();
    let destroy: Vec<String> = log.events()[4..].to_vec();
    let expected: Vec<String> = init.iter().rev().map(|e| e.replace(" init", " destroy")).collect();
    assert_eq!(destroy, expected, "shutdown runs the exact reverse of the startup order");
}

struct Flaky;

#[tokio::test]
async fn a_closure_hook_whose_parameter_read_fails_never_runs_and_is_errored() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.try_transient(|| async { Err::<Flaky, _>(Boom::new("cannot build")) });
        let log = l.clone();
        m.singleton(|| async { S1 }).on_init(move |_f: Dep<Flaky>| {
            let log = log.clone();
            async move {
                log.push("hook body");
                Ok::<(), Boom>(())
            }
        });
    }))
    .await;

    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    assert_eq!((hook, key.key()), (HookKind::OnModuleInit, Key::of::<S1, ()>()));
    let FailureReason::Errored(source) = reason else { panic!("a failed read is `Errored`, got: {reason}") };
    assert!(
        matches!(source.downcast_ref::<LookupError>(), Some(LookupError::Construct { .. })),
        "the failed read is the `LookupError` the hook's parameter produced"
    );
    assert!(log.events().is_empty(), "the hook's body never ran");
}
