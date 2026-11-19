//! The teardown of a connect walk that fails (section 9.2): the `OnModuleDestroy` hooks of every
//! binding and module whose init hooks completed run in the exact reverse of their startup order,
//! each under its bound and all under `shutdown_timeout`, as at close, and what they record travels
//! beside the failure as `ConnectError::Teardown`. A failed `load` tears down what it started the
//! same way.
//!
//! Timeouts run on `Frozen`, whose every sleep ends at once.

#[macro_use]
mod common;

use std::time::Duration;

use common::*;
use ulo::{
    ConnectError, FailureReason, HookKind, Key, Limit, LoadError, ModuleDef, ShutdownFailure, StartupError,
};

struct A;
struct L1;
struct S1;
struct S2;
struct S3;
struct S4;

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
    let StartupError::Connect(ConnectError::Hook { hook, key, .. }) = error else {
        panic!("a teardown that records nothing leaves the failure as it was, got: {error}");
    };
    assert_eq!((hook, key.key()), (HookKind::OnModuleInit, Key::of::<S3, ()>()));
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
        matches!(error, StartupError::Connect(ConnectError::Hook { hook: HookKind::OnApplicationBootstrap, .. })),
        "got: {error}"
    );
    assert_eq!(destroyed(&log), ["module destroy", "s2 destroy", "s1 destroy"], "every init stage completed");
}

#[tokio::test]
async fn a_construction_failure_runs_no_destroy_hook() {
    let log = Log::new();
    let l = log.clone();
    let error = connect_err(Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 }).on_init(init(&l, "s1", false)).on_destroy(destroy(&l, "s1"));
        m.try_singleton(|| async { Err::<S2, _>(Boom::new("down")) });
        m.on_destroy(destroy(&l, "module"));
    }))
    .await;
    assert!(matches!(error, StartupError::Connect(ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(log.events(), Vec::<String>::new(), "no init hook ran, so none completed");
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
    let StartupError::Connect(connect) = &error else { panic!("expected a connect failure, got: {error}") };
    let ConnectError::Hook { hook, key, .. } = connect.failure() else { panic!("the failure is the init hook's, got: {connect}") };
    assert_eq!((*hook, key.key()), (HookKind::OnModuleInit, Key::of::<S4, ()>()));
    let [ShutdownFailure::Hook { hook, key, reason }] = connect.teardown() else {
        panic!("one destroy hook failed, got: {:?}", connect.teardown());
    };
    assert_eq!((*hook, key.key()), (HookKind::OnModuleDestroy, Key::of::<S2, ()>()));
    assert!(matches!(reason, FailureReason::Panicked(_)), "got: {reason}");
    assert!(matches!(connect, ConnectError::Teardown { .. }));
    assert_eq!(destroyed(&log), ["s3 destroy", "s1 destroy"], "the hooks on either side of the panicking one ran");
    let text = error.to_string();
    assert!(text.starts_with("`OnModuleInit` hook of `S4` failed"), "the failure reads first: {text}");
    assert!(text.contains("the destroy hooks run for what had started recorded 1 failure"), "{text}");
    assert!(text.contains("`OnModuleDestroy` hook of `S2`: panicked"), "{text}");
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
    let StartupError::Connect(connect) = &error else { panic!("expected a connect failure, got: {error}") };
    let [ShutdownFailure::Hook { key, reason: FailureReason::TimedOut { after, limit }, .. }] = connect.teardown() else {
        panic!("the destroy hook timed out, got: {:?}", connect.teardown());
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
    let StartupError::Connect(connect) = &error else { panic!("expected a connect failure, got: {error}") };
    let [ShutdownFailure::Hook { key, reason: FailureReason::Skipped, .. }] = connect.teardown() else {
        panic!("the destroy hook was skipped, got: {:?}", connect.teardown());
    };
    assert_eq!(key.key(), Key::of::<S1, ()>());
    assert_eq!(destroyed(&log), Vec::<String>::new());
}

#[tokio::test]
async fn a_failed_load_runs_the_destroy_hooks_of_what_it_initialized() {
    let log = Log::new();
    let app = connects(Empty).await;
    let handle = app.handle();
    let error = handle.load(Mod::<L1>::new(fails_at_s3(&log))).await.err().expect("the load succeeded");
    assert!(matches!(error, LoadError::Connect(ConnectError::Hook { hook: HookKind::OnModuleInit, .. })), "got: {error}");
    assert_eq!(log.events(), ["s1 init", "s2 init", "s3 init", "s2 destroy", "s1 destroy"]);
    close(app).await;
    assert_eq!(destroyed(&log), ["s2 destroy", "s1 destroy"], "the app's own close does not run them again");
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
    assert!(matches!(error, StartupError::Connect(ConnectError::Construct { .. })), "got: {error}");
    assert_eq!(destroyed(&log), ["s2 destroy", "s1 destroy"], "the source is read once every init stage completed");
}
