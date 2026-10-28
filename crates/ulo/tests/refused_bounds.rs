//! The core's own bounds on a clock that refuses them, its runtime shut down (section 3.9): a
//! hook's, a construction's, a readiness check's, the shutdown cap and a transport's `close`
//! bound. A refused bound ends its item as an expired one does and is reported as the refusal,
//! `Errored` holding a `Permanent` `RuntimeShutDown`, under the identity the item's failure
//! names. The ties: an answer ready in the poll after the refusal keeps it, and a refusal that
//! arrives once the clock reads the bound as passed reports the expiry.
//!
//! Clocks are `ScriptedTimer`s: a refused length answers `Err(Refused)` at once, and every other
//! sleep stays pending, so nothing here waits on time. [`Late`] refuses a length only once it is
//! due.

#[macro_use]
mod common;

use std::error::Error;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};

use common::*;
use ulo::{
    App, AppBuilder, BoxFuture, ConnectError, Dep, ExecOptions, FailureReason, HookKind, Key, Limit, LookupError, Module,
    Permanent, Refused, RuntimeShutDown, ShutdownFailure, Signal, StartupError, Timer, TypeName, is_permanent,
};
use ulo_conformance_support::ScriptedTimer;
use ulo_runtime_tokio::Tokio;

struct Root;
struct N;
struct K;
struct Replica;
struct S1;
struct S2;

const BOUND: Duration = Duration::from_secs(5);
const CAP: Duration = Duration::from_secs(9);
const HOOK_DEFAULT: Duration = Duration::from_secs(4);

/// A clock refusing `length`, its other sleeps pending.
fn refusing(length: Duration) -> ScriptedTimer {
    let clock = ScriptedTimer::new();
    clock.refuse(length);
    clock
}

/// [`ScriptedTimer`] whose wait of `late` is refused only once it is due: the clock moves past
/// `late` as the wait is asked, as a runtime blocked past a deadline and then shut down leaves it.
#[derive(Clone)]
struct Late {
    clock: ScriptedTimer,
    late: Duration,
}

impl Late {
    fn new(late: Duration) -> Late {
        Late { clock: ScriptedTimer::new(), late }
    }
}

impl Timer for Late {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        if d == self.late {
            self.clock.advance(d);
            return Box::pin(std::future::ready(Err(Refused)));
        }
        self.clock.try_sleep(d)
    }
}

/// The `RuntimeShutDown` an item ended with, found where the core stores it: inside the
/// `Permanent` its `Errored` reason holds, the runtime's `Refused` as its source.
fn shut_down(reason: &FailureReason) -> &RuntimeShutDown {
    let FailureReason::Errored(stored) = reason else {
        panic!("the item did not end as `Errored`: {reason:?}");
    };
    let permanent = stored.downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the item's error is not `Permanent`: {reason:?}"));
    let ended = permanent
        .source()
        .and_then(|inner| inner.downcast_ref::<RuntimeShutDown>())
        .unwrap_or_else(|| panic!("the `Permanent` does not hold a `RuntimeShutDown`: {reason:?}"));
    assert!(ended.source().is_some_and(|e| e.is::<Refused>()), "the refusal's source is not the runtime's `Refused`: {ended:?}");
    ended
}

fn timed_out(reason: &FailureReason) -> (Duration, Limit) {
    match reason {
        FailureReason::TimedOut { after, limit } => (*after, *limit),
        other => panic!("expected the bound's expiry, got: {other:?}"),
    }
}

/// The startup error of `root` on `clock`, whose connect must fail.
async fn connect_failing(root: impl Module, clock: impl Timer) -> StartupError {
    watchdog();
    let app = App::builder(root).timer(clock).wire().expect("the graph did not wire");
    within("connect", app.connect()).await.err().expect("connect succeeded on a bound that cannot pass")
}

/// Pending on its first poll, waking itself, then `Ok`.
async fn answers_on_the_second_poll() -> Result<(), Boom> {
    let mut polled = false;
    std::future::poll_fn(|cx| {
        if polled {
            return Poll::Ready(Ok(()));
        }
        polled = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await
}

// ---- A hook ----

/// A binding's init hook in an imported module: the hook kind and the binding's key.
fn pending_init_hook() -> Mod<Root> {
    Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.singleton(|| async { S1 }).on_init(std::future::pending::<Result<(), Boom>>).timeout(BOUND);
        }));
    })
}

fn hook_failure(error: &StartupError) -> (HookKind, Key, &FailureReason) {
    let StartupError::Connect(ConnectError::Hook { hook, key, reason }) = error else {
        panic!("expected a hook failure, got: {error}");
    };
    (*hook, key.key(), reason)
}

#[tokio::test]
async fn a_refused_hook_bound_fails_the_hook_with_the_refusal() {
    let error = connect_failing(pending_init_hook(), refusing(BOUND)).await;
    assert!(is_permanent(&error), "a hook ended on a refused bound is retryable: {error}");
    let (hook, key, reason) = hook_failure(&error);
    assert_eq!((hook, key), (HookKind::OnModuleInit, Key::of::<S1, ()>()));
    shut_down(reason);
}

#[tokio::test]
async fn a_refused_module_hook_bound_names_the_keyed_module() {
    let root = Mod::<Root>::new(|m| {
        m.import(
            Mod::<K>::new(|m| {
                m.on_init(std::future::pending::<Result<(), Boom>>).timeout(BOUND);
            })
            .keyed::<Replica>(),
        );
    });
    let error = connect_failing(root, refusing(BOUND)).await;
    let (hook, key, reason) = hook_failure(&error);
    assert_eq!((hook, key), (HookKind::OnModuleInit, Key::of::<Mod<K>, Replica>()));
    shut_down(reason);
}

#[tokio::test]
async fn a_hook_bound_refused_after_it_passed_reports_the_expiry() {
    let error = connect_failing(pending_init_hook(), Late::new(BOUND)).await;
    assert!(!is_permanent(&error), "an expired hook bound is permanent: {error}");
    let (hook, key, reason) = hook_failure(&error);
    assert_eq!((hook, key), (HookKind::OnModuleInit, Key::of::<S1, ()>()));
    assert_eq!(timed_out(reason), (BOUND, Limit::Item));
}

/// The first tie: a hook pending when its bound is refused and ready on the next poll keeps its
/// answer, and connect succeeds.
#[tokio::test]
async fn a_hook_ready_in_the_poll_after_its_bound_is_refused_succeeds() {
    watchdog();
    let root = Mod::<Root>::new(|m| {
        m.singleton(|| async { S1 }).on_init(answers_on_the_second_poll).timeout(BOUND);
    });
    let app = App::builder(root).timer(refusing(BOUND)).wire().expect("the graph did not wire");
    let app = within("connect", app.connect()).await.unwrap_or_else(|error| panic!("the hook's answer lost to the refusal: {error}"));
    close(app).await;
}

// ---- A construction ----

fn pending_singleton() -> Mod<Root> {
    Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.singleton(std::future::pending::<S1>).timeout(BOUND);
        }));
    })
}

fn construct_failure(error: &StartupError) -> (Key, String, &FailureReason) {
    let StartupError::Connect(ConnectError::Construct { key, module, reason }) = error else {
        panic!("expected a construction failure, got: {error}");
    };
    (key.key(), module.to_string(), reason)
}

#[tokio::test]
async fn a_refused_construction_bound_fails_the_construction_with_the_refusal() {
    let error = connect_failing(pending_singleton(), refusing(BOUND)).await;
    assert!(is_permanent(&error), "a construction ended on a refused bound is retryable: {error}");
    let (key, module, reason) = construct_failure(&error);
    assert_eq!((key, module), (Key::of::<S1, ()>(), mod_name::<N>()));
    shut_down(reason);
}

#[tokio::test]
async fn a_construction_bound_refused_after_it_passed_reports_the_expiry() {
    let error = connect_failing(pending_singleton(), Late::new(BOUND)).await;
    let (key, module, reason) = construct_failure(&error);
    assert_eq!((key, module), (Key::of::<S1, ()>(), mod_name::<N>()));
    assert_eq!(timed_out(reason), (BOUND, Limit::Item));
}

/// An execution-scoped construction inside a call, `LookupError::Construct` naming its key.
async fn build_in_a_call(clock: impl Timer) -> LookupError {
    watchdog();
    let root = Mod::<Root>::new(|m| {
        m.execution(std::future::pending::<S2>).timeout(BOUND);
    });
    let app = App::builder(root).timer(clock).wire().expect("the graph did not wire");
    let app = within("connect", app.connect()).await.unwrap_or_else(|error| panic!("connect failed: {error}"));
    let read = within("the call", app.execute(ExecOptions::new(), async |exec| exec.get::<S2>().await.map(|_| ()))).await;
    let error = read.expect("a connected app opens executions").expect_err("a construction that never ends was read");
    close(app).await;
    error
}

#[tokio::test]
async fn a_refused_construction_bound_inside_a_call_fails_the_read_with_the_refusal() {
    let LookupError::Construct { key, reason } = build_in_a_call(refusing(BOUND)).await else {
        panic!("expected a construction failure inside the call");
    };
    assert_eq!(key.key(), Key::of::<S2, ()>());
    shut_down(&reason);
}

#[tokio::test]
async fn a_construction_bound_inside_a_call_refused_after_it_passed_reports_the_expiry() {
    let LookupError::Construct { key, reason } = build_in_a_call(Late::new(BOUND)).await else {
        panic!("expected a construction failure inside the call");
    };
    assert_eq!(key.key(), Key::of::<S2, ()>());
    assert_eq!(timed_out(&reason), (BOUND, Limit::Item));
}

// ---- A readiness check ----

const BACKOFF: Duration = Duration::from_millis(7);

/// A check that never answers, retried with a backoff, under an attempt bound of `BOUND`.
fn pending_check_per_attempt() -> Mod<Root> {
    Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.singleton(|| async { S1 })
                .ready(|_s: Dep<S1>| std::future::pending::<Result<(), Boom>>())
                .retries(3)
                .backoff(BACKOFF)
                .attempt_timeout(BOUND);
        }));
    })
}

/// A check that never answers, under a whole-check bound of `BOUND`.
fn pending_check_whole() -> Mod<Root> {
    Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.singleton(|| async { S1 }).ready(|_s: Dep<S1>| std::future::pending::<Result<(), Boom>>()).timeout(BOUND);
        }));
    })
}

fn readiness_failure(error: &StartupError) -> (Key, u32, &FailureReason) {
    let StartupError::Connect(ConnectError::Readiness { key, attempts, reason }) = error else {
        panic!("expected a readiness failure, got: {error}");
    };
    (key.key(), *attempts, reason)
}

/// A refused attempt bound ends the check at once, as a `Permanent` attempt error does: no
/// backoff and no further attempt.
#[tokio::test]
async fn a_refused_attempt_bound_ends_the_check_at_once_with_the_refusal() {
    let clock = refusing(BOUND);
    let error = connect_failing(pending_check_per_attempt(), clock.clone()).await;
    assert!(is_permanent(&error), "a check ended on a refused bound is retryable: {error}");
    let (key, attempts, reason) = readiness_failure(&error);
    assert_eq!((key, attempts), (Key::of::<S1, ()>(), 1));
    assert_eq!((clock.refused(BOUND), clock.live(BACKOFF)), (1, 0), "the attempt bounds asked, and the backoffs waiting");
    shut_down(reason);
}

#[tokio::test]
async fn an_attempt_bound_refused_after_it_passed_reports_the_expiry() {
    let late = Late::new(BOUND);
    let clock = late.clock.clone();
    let error = within("connect", async {
        watchdog();
        let app = App::builder(pending_check_per_attempt()).timer(late).wire().expect("the graph did not wire");
        let connecting = app.connect();
        // Each expired attempt is retried after the backoff, which the scripted clock times: it is
        // moved on whenever a backoff waits.
        let mut connecting = std::pin::pin!(connecting);
        loop {
            tokio::select! {
                ended = &mut connecting => break ended.err().expect("connect succeeded on a check that never answers"),
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    if clock.live(BACKOFF) > 0 {
                        clock.advance(BACKOFF);
                    }
                }
            }
        }
    })
    .await;
    let (key, attempts, reason) = readiness_failure(&error);
    assert_eq!((key, attempts), (Key::of::<S1, ()>(), 4), "an expired attempt is retried");
    assert_eq!(timed_out(reason), (BOUND, Limit::Attempt));
}

#[tokio::test]
async fn a_refused_whole_check_bound_fails_the_check_with_the_refusal() {
    let error = connect_failing(pending_check_whole(), refusing(BOUND)).await;
    assert!(is_permanent(&error), "a check ended on a refused bound is retryable: {error}");
    let (key, attempts, reason) = readiness_failure(&error);
    assert_eq!((key, attempts), (Key::of::<S1, ()>(), 1));
    shut_down(reason);
}

#[tokio::test]
async fn a_whole_check_bound_refused_after_it_passed_reports_the_expiry() {
    let error = connect_failing(pending_check_whole(), Late::new(BOUND)).await;
    let (key, attempts, reason) = readiness_failure(&error);
    assert_eq!((key, attempts), (Key::of::<S1, ()>(), 1));
    assert_eq!(timed_out(reason), (BOUND, Limit::Item));
}

// ---- A shutdown hook, its own bound and the cap ----

/// Connects `root` on `builder` and closes it, answering the failures the shutdown reported.
async fn shutdown_failures(builder: AppBuilder) -> Arc<[ShutdownFailure]> {
    watchdog();
    let app = builder.wire().expect("the graph did not wire");
    let app = within("connect", app.connect()).await.unwrap_or_else(|error| panic!("connect failed: {error}"));
    let error = within("close", app.close(Signal::new("done"))).await.expect_err("shutdown reported no failure");
    error.failures
}

fn hook_cut(failures: &[ShutdownFailure]) -> (HookKind, Key, &FailureReason) {
    let [ShutdownFailure::Hook { hook, key, reason }] = failures else {
        panic!("expected one hook failure, got: {failures:?}");
    };
    (*hook, key.key(), reason)
}

/// A destroy hook in an imported module that never ends, under its own bound of `BOUND`.
fn pending_destroy_hook() -> Mod<Root> {
    Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.singleton(|| async { S1 }).on_destroy(std::future::pending::<()>).timeout(BOUND);
        }));
    })
}

/// A destroy hook in an imported module that never ends and has no bound of its own.
fn unbounded_destroy_hook() -> Mod<Root> {
    Mod::<Root>::new(|m| {
        m.import(Mod::<N>::new(|m| {
            m.singleton(|| async { S1 }).on_destroy(std::future::pending::<()>).unbounded();
        }));
    })
}

#[tokio::test]
async fn a_refused_shutdown_hook_bound_fails_the_hook_with_the_refusal() {
    let failures = shutdown_failures(App::builder(pending_destroy_hook()).timer(refusing(BOUND))).await;
    let (hook, key, reason) = hook_cut(&failures);
    assert_eq!((hook, key), (HookKind::OnModuleDestroy, Key::of::<S1, ()>()));
    shut_down(reason);
}

#[tokio::test]
async fn a_shutdown_hook_bound_refused_after_it_passed_reports_the_expiry() {
    let failures = shutdown_failures(App::builder(pending_destroy_hook()).timer(Late::new(BOUND))).await;
    let (hook, key, reason) = hook_cut(&failures);
    assert_eq!((hook, key), (HookKind::OnModuleDestroy, Key::of::<S1, ()>()));
    assert_eq!(timed_out(reason), (BOUND, Limit::Item));
}

/// A refused cap cuts the hook at once, as a spent cap does, never leaving it unbounded.
#[tokio::test]
async fn a_refused_cap_cuts_a_running_hook_with_the_refusal() {
    let failures = shutdown_failures(App::builder(unbounded_destroy_hook()).timer(refusing(CAP)).shutdown_timeout(CAP)).await;
    let (hook, key, reason) = hook_cut(&failures);
    assert_eq!((hook, key), (HookKind::OnModuleDestroy, Key::of::<S1, ()>()));
    shut_down(reason);
}

#[tokio::test]
async fn a_cap_refused_after_it_passed_reports_the_cap() {
    let failures = shutdown_failures(App::builder(unbounded_destroy_hook()).timer(Late::new(CAP)).shutdown_timeout(CAP)).await;
    let (hook, key, reason) = hook_cut(&failures);
    assert_eq!((hook, key), (HookKind::OnModuleDestroy, Key::of::<S1, ()>()));
    assert_eq!(timed_out(reason), (CAP, Limit::ShutdownCap));
}

// ---- A transport's `close` ----

/// Binds one server whose `close` never ends, serves, and stops: the failures and the log.
async fn close_failures(builder: AppBuilder) -> (Arc<[ShutdownFailure]>, Log) {
    let log = Log::new();
    let app = listened(builder, vec![server(TestServer::<TA>::new("c", &log).close_with(CloseMode::Pending))])
        .await
        .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let error = within("serve", app.serve(async { Signal::new("done") })).await.expect_err("shutdown reported no failure");
    (error.failures, log)
}

fn close_cut(failures: &[ShutdownFailure]) -> &FailureReason {
    let [ShutdownFailure::Close { transport, reason }] = failures else {
        panic!("expected one close failure, got: {failures:?}");
    };
    assert_eq!(*transport, TypeName::of::<TA>());
    reason
}

/// `runtime` is set first and `timer` after, so the core times on `clock`.
fn on(clock: impl Timer) -> AppBuilder {
    App::builder(Empty).runtime(Tokio::current()).timer(clock)
}

#[tokio::test]
async fn a_refused_close_bound_fails_the_close_with_the_refusal() {
    let (failures, log) = close_failures(on(refusing(HOOK_DEFAULT)).hook_timeout(HOOK_DEFAULT)).await;
    shut_down(close_cut(&failures));
    assert!(log.has("close c"), "the close never started: {:?}", log.events());
}

#[tokio::test]
async fn a_close_bound_refused_after_it_passed_reports_the_expiry() {
    let (failures, _) = close_failures(on(Late::new(HOOK_DEFAULT)).hook_timeout(HOOK_DEFAULT)).await;
    assert_eq!(timed_out(close_cut(&failures)), (HOOK_DEFAULT, Limit::Default));
}

/// A refused cap cuts a `close` as a spent cap does: its first poll runs, and the step ends.
#[tokio::test]
async fn a_refused_cap_cuts_a_close_with_the_refusal() {
    let (failures, log) = close_failures(on(refusing(CAP)).shutdown_timeout(CAP)).await;
    shut_down(close_cut(&failures));
    assert!(log.has("close c"), "the close never started: {:?}", log.events());
}

#[tokio::test]
async fn a_cap_on_a_close_refused_after_it_passed_reports_the_cap() {
    let (failures, _) = close_failures(on(Late::new(CAP)).shutdown_timeout(CAP)).await;
    assert_eq!(timed_out(close_cut(&failures)), (CAP, Limit::ShutdownCap));
}
