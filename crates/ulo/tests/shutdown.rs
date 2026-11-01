//! The shutdown sequence (section 9.5) with servers: its order, its three triggers, what a failing
//! step records, what the cap does, and how the drain treats live and terminal executions.
//!
//! Capabilities pinned: 39 and 40 for the close side (before-shutdown, drain, destroy, every
//! transport's `close` at once with its failures reported in bind order, on-shutdown), and the close rows of section 3: a
//! shutdown hook that panics, times out, is skipped by the cap or whose parameter read fails; a
//! transport `close` that errs, panics or exceeds its bound; later steps still run. A server's
//! `close` is a promise: it runs once on every shutdown path that reaches the close step.
//!
//! Already pinned by `ulo-runtime-tokio/tests/app.rs`, and not repeated: a transport's `serve`
//! that fails or returns while the app runs starts the shutdown and `serve` answers
//! `ShutdownFailure::Serve` (one failing server beside a healthy one, two ending on one poll, one
//! ending after a `close`), and every release of the servers by a drop (`ShutdownFailure::Dropped`,
//! `draining()` waking a loop). The plan's two-server serve-death scenario is
//! `a_transport_whose_serve_fails_closes_the_app` there. A hook that panics in the destroy step and
//! a `LookupError::Closed` read from a destroy hook are in `phases.rs` and `lookup.rs`.
//!
//! Bounds on `Frozen` end every sleep at once, so a pending item is abandoned in the poll that
//! starts it. The drain tests use the real clock with short windows and wait on the real clock
//! under `within`; no test waits on a frozen timer.

#[macro_use]
mod common;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use common::*;
use ulo::{
    App, AppHandle, CancelReason, Dep, ExecOptions, Execution, FailureReason, HookKind, Key, Limit, LookupError, ModuleDef,
    ShutdownFailure, Signal, Transport, TypeName,
};
use ulo_runtime_tokio::Tokio;

struct A;
struct S1;
struct S2;
struct S3;
struct S4;
struct Flaky;

struct TC;

impl Transport for TC {
    const KEY: &'static str = "c";
    type Cx = ();
    type Reply = ();
}

struct TD;

impl Transport for TD {
    const KEY: &'static str = "d";
    type Cx = ();
    type Reply = ();
}

macro_rules! log_destroy {
    ($log:expr, $text:expr) => {{
        let log = $log.clone();
        move || {
            let log = log.clone();
            async move { log.push($text) }
        }
    }};
}

macro_rules! log_signal {
    ($log:expr, $text:expr) => {{
        let log = $log.clone();
        move |signal: Signal| {
            let log = log.clone();
            async move { log.push(format!("{} {}", $text, signal.name())) }
        }
    }};
}

/// Waits until `log` holds `event`, on the real clock.
async fn until(log: &Log, event: &str) {
    let deadline = Instant::now() + PATIENCE;
    while !log.has(event) {
        assert!(Instant::now() < deadline, "`{event}` was never logged: {:?}", log.events());
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn frozen(root: impl ulo::Module) -> ulo::AppBuilder {
    App::builder(root).runtime(Tokio::current()).timer(Frozen::new())
}

#[tokio::test]
async fn the_sequence_runs_before_shutdown_drain_destroy_close_then_shutdown_and_servers_close_in_reverse_bind_order() {
    let log = Log::new();
    let l = log.clone();
    let app = listened(
        App::builder(Mod::<A>::new(move |m| {
            m.singleton(|| async { S1 })
                .before_shutdown(log_signal!(l, "before-shutdown"))
                .on_destroy(log_destroy!(l, "destroy"))
                .on_shutdown(log_signal!(l, "on-shutdown"));
        }))
        .runtime(Tokio::current()),
        vec![server(TestServer::<TA>::new("a", &log)), server(TestServer::<TB>::new("b", &log))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));

    let report = within("serve", app.serve(async { Signal::new("done") })).await.expect("shutdown reported a failure");
    assert_eq!(report.signal.name(), "done");
    assert_eq!((report.abandoned, report.terminal_skipped), (0, 0));
    assert_eq!(
        log.events(),
        [
            "prepare a",
            "prepare b",
            "bind a",
            "bind b",
            "before-shutdown done",
            "drain a",
            "drain b",
            "destroy",
            "close b",
            "close a",
            "on-shutdown done",
        ],
        "hooks run while the app serves, every server drains, destroy runs, servers close last bound first, then the shutdown hooks"
    );
}

#[tokio::test]
async fn a_close_on_a_handle_ends_serve_and_its_signal_is_the_one_the_hooks_receive() {
    let log = Log::new();
    let l = log.clone();
    let app = listened(
        App::builder(Mod::<A>::new(move |m| {
            m.singleton(|| async { S1 }).before_shutdown(log_signal!(l, "before-shutdown"));
        }))
        .runtime(Tokio::current()),
        vec![server(TestServer::<TA>::new("a", &log))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));

    let closed = within("close", handle.close(Signal::new("operator"))).await.expect("shutdown reported a failure");
    let served = within("serve", serving).await.expect("the serving task panicked").expect("shutdown reported a failure");
    assert_eq!((closed.signal.name(), served.signal.name()), ("operator", "operator"));
    assert!(log.has("before-shutdown operator"), "{:?}", log.events());
}

#[tokio::test]
async fn a_close_during_a_running_shutdown_waits_for_it_and_receives_its_outcome_under_its_signal() {
    let log = Log::new();
    let gate = Gate::new(false);
    let (l, g) = (log.clone(), Arc::clone(&gate));
    let app = connects(gated_root(l, g)).await;
    let handle = app.handle();

    let first = tokio::spawn(handle.close(Signal::new("first")));
    until(&log, "hook started").await;
    let second = tokio::spawn(handle.close(Signal::new("second")));
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(!second.is_finished(), "a second close answers when the running shutdown does");
    gate.open();

    let first = within("first close", first).await.unwrap().expect_err("the destroy hook's panic was not reported");
    let second = within("second close", second).await.unwrap().expect_err("the destroy hook's panic was not reported");
    assert_eq!((first.report.signal.name(), second.report.signal.name()), ("first", "first"), "the second close reports the shutdown's own signal");
    assert!(Arc::ptr_eq(&first.failures, &second.failures), "one outcome, shared by every receiver");
    assert!(
        matches!(&*first.failures, [ShutdownFailure::Hook { hook: HookKind::OnModuleDestroy, reason: FailureReason::Panicked(_), .. }]),
        "got: {first}"
    );
    assert!(ulo::is_panic(&ulo::BoxError::from(first.clone())), "`is_panic` answers true for a hook whose reason is `Panicked`");
    assert_eq!(log.count("hook started"), 1, "the shutdown ran once");
    drop(app);
}

fn gated_root(log: Log, gate: Arc<Gate>) -> Mod<A> {
    Mod::new(move |m| {
        let (log, gate) = (log.clone(), Arc::clone(&gate));
        m.singleton(|| async { S1 }).before_shutdown(move |_s: Signal| {
            let (log, gate) = (log.clone(), Arc::clone(&gate));
            async move {
                log.push("hook started");
                gate.pass().await;
            }
        });
        m.singleton(|| async { S2 }).on_destroy(|| async {
            if true {
                panic!("the destroy hook panics");
            }
        });
    })
}

#[tokio::test]
async fn a_shutdown_hook_that_panics_times_out_or_cannot_read_its_parameter_is_recorded_and_the_sequence_goes_on() {
    watchdog();
    let log = Log::new();
    let l = log.clone();
    let app = frozen(Mod::<A>::new(move |m| {
        // The last hook of the last step, declared first: it runs last.
        m.singleton(|| async { S4 }).on_shutdown(log_signal!(l, "last"));
        m.try_transient(|| async { Err::<Flaky, _>(Boom::new("cannot build")) });
        m.singleton(|| async { S3 }).on_shutdown({
            let log = l.clone();
            move |_s: Signal, _f: Dep<Flaky>| {
                let log = log.clone();
                async move { log.push("s3 hook body") }
            }
        });
        m.singleton(|| async { S2 })
            .on_destroy(|| async {
                std::future::pending::<()>().await;
            })
            .timeout(Duration::from_secs(3));
        m.singleton(|| async { S1 }).before_shutdown(|_s: Signal| async {
            if true {
                panic!("the hook panics");
            }
        });
    }))
    .wire()
    .unwrap()
    .connect()
    .await
    .expect("connect failed");

    let error = within("close", app.close(Signal::new("done"))).await.expect_err("shutdown reported no failure");
    let [first, second, third] = &*error.failures else { panic!("expected three failures, got: {error}") };
    match first {
        ShutdownFailure::Hook { hook, key, reason } => {
            assert_eq!((*hook, key.key()), (HookKind::BeforeApplicationShutdown, Key::of::<S1, ()>()));
            assert!(matches!(reason, FailureReason::Panicked(_)), "got: {reason}");
        }
        other => panic!("expected a hook failure, got: {other}"),
    }
    match second {
        ShutdownFailure::Hook { hook, key, reason: FailureReason::TimedOut { after, limit } } => {
            assert_eq!((*hook, key.key()), (HookKind::OnModuleDestroy, Key::of::<S2, ()>()));
            assert_eq!((*after, *limit), (Duration::from_secs(3), Limit::Item));
        }
        other => panic!("expected a hook timeout, got: {other}"),
    }
    match third {
        ShutdownFailure::Hook { hook, key, reason: FailureReason::Errored(source) } => {
            assert_eq!((*hook, key.key()), (HookKind::OnApplicationShutdown, Key::of::<S3, ()>()));
            assert!(
                matches!(source.downcast_ref::<LookupError>(), Some(LookupError::Construct { .. })),
                "the failed read is the `LookupError` the parameter produced"
            );
        }
        other => panic!("expected a failed read, got: {other}"),
    }
    assert_eq!(log.events(), ["last done"], "the hook whose read failed never ran, and the hook after the failures did");
}

#[tokio::test]
async fn an_expired_cap_skips_every_hook_not_yet_run_and_closes_a_server_that_cannot_finish_at_once() {
    let log = Log::new();
    let l = log.clone();
    let root = Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 })
            .before_shutdown(log_signal!(l, "before-shutdown"))
            .on_destroy(log_destroy!(l, "destroy"))
            .on_shutdown(log_signal!(l, "on-shutdown"));
    });
    let app = listened(
        frozen(root).shutdown_timeout(Duration::ZERO),
        vec![
            server(TestServer::<TA>::new("a", &log)),
            server(TestServer::<TB>::new("b", &log).close_with(CloseMode::Pending)),
        ],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));

    let error = within("serve", app.serve(async { Signal::new("done") })).await.expect_err("shutdown reported no failure");
    let [h1, h2, close, h3] = &*error.failures else { panic!("expected four failures, got: {error}") };
    for (failure, kind) in [
        (h1, HookKind::BeforeApplicationShutdown),
        (h2, HookKind::OnModuleDestroy),
        (h3, HookKind::OnApplicationShutdown),
    ] {
        match failure {
            ShutdownFailure::Hook { hook, reason, .. } => {
                assert_eq!(*hook, kind);
                assert!(matches!(reason, FailureReason::Skipped), "got: {reason}");
            }
            other => panic!("expected a skipped hook, got: {other}"),
        }
    }
    match close {
        ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } } => {
            assert_eq!(*transport, TypeName::of::<TB>());
            assert_eq!((*after, *limit), (Duration::ZERO, Limit::ShutdownCap));
        }
        other => panic!("expected a close timeout, got: {other}"),
    }
    assert_eq!(
        log.events().iter().filter(|e| !e.starts_with("prepare") && !e.starts_with("bind")).collect::<Vec<_>>(),
        ["drain a", "drain b", "close b", "close a"],
        "no hook ran, every server entered its drain, and every `close` still started, the one that finished on its first poll without a failure"
    );
}

/// DESIGN section 9.5: "Stop accepting still runs, as it runs no user code", and an open drain ends
/// at once. A server's `drain` is where its transport stops accepting; with the cap expired the
/// core creates each `drain` future and drops it without a poll, so the transport's own stop
/// never runs. F455.
#[tokio::test]
async fn an_expired_cap_still_enters_each_servers_drain() {
    let log = Log::new();
    let app = listened(
        frozen(Empty).shutdown_timeout(Duration::ZERO),
        vec![server(TestServer::<TA>::new("a", &log)), server(TestServer::<TB>::new("b", &log))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let _ = within("serve", app.serve(async { Signal::new("done") })).await;
    assert!(log.has("drain a") && log.has("drain b"), "a server's `drain` never started: {:?}", log.events());
}

#[tokio::test]
async fn a_hook_still_running_when_the_cap_runs_out_is_dropped_with_the_caps_limit_whatever_its_own_bound() {
    watchdog();
    let app = frozen(Mod::<A>::new(|m| {
        m.singleton(|| async { S1 })
            .before_shutdown(|_s: Signal| async {
                std::future::pending::<()>().await;
            })
            .timeout(Duration::from_secs(1))
            .on_destroy(|| async {
                std::future::pending::<()>().await;
            });
    }))
    .shutdown_timeout(Duration::from_secs(10))
    .wire()
    .unwrap()
    .connect()
    .await
    .expect("connect failed");

    let error = within("close", app.close(Signal::new("done"))).await.expect_err("shutdown reported no failure");
    assert_eq!(error.failures.len(), 2, "{error}");
    for (failure, kind) in error.failures.iter().zip([HookKind::BeforeApplicationShutdown, HookKind::OnModuleDestroy]) {
        match failure {
            ShutdownFailure::Hook { hook, reason: FailureReason::TimedOut { after, limit }, .. } => {
                assert_eq!(*hook, kind);
                assert_eq!((*after, *limit), (Duration::from_secs(10), Limit::ShutdownCap), "the cap wins over the hook's own bound");
            }
            other => panic!("expected a hook cut by the cap, got: {other}"),
        }
    }
}

fn four_servers(log: &Log) -> Vec<Queued> {
    vec![
        server(TestServer::<TA>::new("a", log)),
        server(TestServer::<TB>::new("b", log).close_with(CloseMode::Err)),
        server(TestServer::<TC>::new("c", log).close_with(CloseMode::Panic)),
        server(TestServer::<TD>::new("d", log).close_with(CloseMode::Pending)),
    ]
}

#[tokio::test]
async fn a_close_that_errs_panics_or_exceeds_its_bound_is_recorded_and_every_close_runs() {
    let log = Log::new();
    let app = listened(frozen(Empty).hook_timeout(Duration::from_secs(4)), four_servers(&log))
        .await
        .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let error = within("serve", app.serve(async { Signal::new("done") })).await.expect_err("shutdown reported no failure");

    let closes: Vec<String> = log.events().into_iter().filter(|e| e.starts_with("close")).collect();
    assert_eq!(closes, ["close d", "close c", "close b", "close a"], "every `close` started, last bound first");
    let [b, c, d] = &*error.failures else { panic!("expected three failures, got: {error}") };
    match d {
        ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } } => {
            assert_eq!(*transport, TypeName::of::<TD>());
            assert_eq!((*after, *limit), (Duration::from_secs(4), Limit::Default), "with no cap, `hook_timeout` bounds a `close`");
        }
        other => panic!("expected a close timeout, got: {other}"),
    }
    assert!(
        matches!(c, ShutdownFailure::Close { transport, reason: FailureReason::Panicked(_) } if *transport == TypeName::of::<TC>()),
        "got: {c}"
    );
    assert!(ulo::is_panic(&ulo::BoxError::from(error.clone())), "`is_panic` answers true when one failure among several is a panicking `close`");
    match b {
        ShutdownFailure::Close { transport, reason: FailureReason::Errored(source) } => {
            assert_eq!(*transport, TypeName::of::<TB>());
            assert!(source.downcast_ref::<Boom>().is_some(), "the transport's own error is reachable");
        }
        other => panic!("expected a close error, got: {other}"),
    }
}

#[tokio::test]
async fn a_close_that_exceeds_the_cap_reports_the_cap_as_its_limit() {
    let log = Log::new();
    let app = listened(frozen(Empty).hook_timeout(Duration::from_secs(4)).shutdown_timeout(Duration::from_secs(8)), four_servers(&log))
        .await
        .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let error = within("serve", app.serve(async { Signal::new("done") })).await.expect_err("shutdown reported no failure");
    let transports: Vec<TypeName> = error
        .failures
        .iter()
        .map(|failure| match failure {
            ShutdownFailure::Close { transport, .. } => *transport,
            other => panic!("expected only close failures, got: {other}"),
        })
        .collect();
    assert_eq!(transports, [TypeName::of::<TB>(), TypeName::of::<TC>(), TypeName::of::<TD>()], "the failures in bind order");
    match &error.failures[2] {
        ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } } => {
            assert_eq!(*transport, TypeName::of::<TD>());
            assert_eq!((*after, *limit), (Duration::from_secs(8), Limit::ShutdownCap), "the cap, not `hook_timeout`, bounded the close");
        }
        other => panic!("expected a close timeout, got: {other}"),
    }
}

/// The closes run at once, so two that each take a while take as long as the longer one, and
/// the report lists their failures in bind order, not in the order they ended. `a` is bound first
/// and ends last.
#[tokio::test]
async fn the_closes_run_at_once_and_their_failures_are_reported_in_bind_order() {
    let log = Log::new();
    let app = listened(
        App::builder(Empty).runtime(Tokio::current()),
        vec![
            server(TestServer::<TA>::new("a", &log).close_with(CloseMode::ErrAfter(Duration::from_millis(400)))),
            server(TestServer::<TB>::new("b", &log).close_with(CloseMode::ErrAfter(Duration::from_millis(300)))),
        ],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    let started = Instant::now();
    let error = within("close", handle.close(Signal::new("done"))).await.expect_err("shutdown reported no failure");
    let took = started.elapsed();
    let _ = within("serve", serving).await;

    assert!(took >= Duration::from_millis(400), "close returned before the longer close ended: {took:?}");
    assert!(took < Duration::from_millis(650), "the closes ran one after another, 700 ms in all: {took:?}");
    let closes: Vec<String> = log.events().into_iter().filter(|e| e.starts_with("close")).collect();
    assert_eq!(closes, ["close b", "close a"], "both closes started, last bound first");
    let failed: Vec<(TypeName, String)> = error
        .failures
        .iter()
        .map(|failure| match failure {
            ShutdownFailure::Close { transport, reason: FailureReason::Errored(source) } => (*transport, source.to_string()),
            other => panic!("expected a close error, got: {other}"),
        })
        .collect();
    assert_eq!(
        failed,
        [(TypeName::of::<TA>(), "close a refused".to_owned()), (TypeName::of::<TB>(), "close b refused".to_owned())],
        "bind order, `a` first though it ended last, each failure under its own transport"
    );
}

#[tokio::test]
async fn the_drain_waits_for_a_live_execution_and_cancels_nothing_inside_the_window() {
    let log = Log::new();
    let app = within_wire(30).await;
    let handle = app.handle();
    let gate = Gate::new(false);
    let working = spawn_worker(&handle, &log, &gate);
    until(&log, "started").await;

    let closing = tokio::spawn(handle.close(Signal::new("done")));
    until(&log, "draining, cancelled false").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!closing.is_finished(), "the shutdown moved on while an execution was still live inside the window");
    gate.open();

    let report = within("close", closing).await.unwrap().expect("shutdown reported a failure");
    assert_eq!(report.abandoned, 0, "the execution ended on its own");
    within("execution", working).await.unwrap().expect("the execution was refused");
    assert!(log.has("finished, cancelled false"), "{:?}", log.events());
    drop(app);
}

/// An app with no servers and a drain window of `secs` seconds on the real clock.
async fn within_wire(secs: u64) -> ulo::App<ulo::Connected> {
    watchdog();
    App::builder(Empty)
        .runtime(Tokio::current())
        .drain_timeout(Duration::from_secs(secs))
        .wire()
        .unwrap()
        .connect()
        .await
        .expect("connect failed")
}

fn spawn_worker(
    handle: &AppHandle,
    log: &Log,
    gate: &Arc<Gate>,
) -> tokio::task::JoinHandle<Result<(), ulo::Closed>> {
    let (handle, log, gate) = (handle.clone(), log.clone(), Arc::clone(gate));
    tokio::spawn(async move {
        handle
            .execute(ExecOptions::new(), async |exec| {
                log.push("started");
                exec.draining().await;
                log.push(format!("draining, cancelled {}", exec.is_cancelled()));
                gate.pass().await;
                log.push(format!("finished, cancelled {}", exec.is_cancelled()));
            })
            .await
    })
}

#[tokio::test]
async fn an_execution_still_live_at_the_drain_timeout_is_cancelled_and_counted_as_abandoned() {
    watchdog();
    let log = Log::new();
    let app = App::builder(Empty)
        .runtime(Tokio::current())
        .drain_timeout(Duration::from_millis(50))
        .wire()
        .unwrap()
        .connect()
        .await
        .expect("connect failed");
    let handle = app.handle();
    let gate = Gate::new(false);
    let reasons: Arc<Mutex<Vec<Option<CancelReason>>>> = Arc::default();
    let working = {
        let (handle, log, gate, reasons) = (handle.clone(), log.clone(), Arc::clone(&gate), Arc::clone(&reasons));
        tokio::spawn(async move {
            handle
                .execute(ExecOptions::new(), async |exec| {
                    log.push("started");
                    exec.cancelled().await;
                    reasons.lock().unwrap_or_else(PoisonError::into_inner).push(exec.cancel_reason());
                    log.push("cancelled");
                    gate.pass().await;
                })
                .await
        })
    };
    until(&log, "started").await;

    let report = within("close", handle.close(Signal::new("done"))).await.expect("shutdown reported a failure");
    assert_eq!(report.abandoned, 1, "the execution outlived the window and is counted");
    until(&log, "cancelled").await;
    assert_eq!(*reasons.lock().unwrap_or_else(PoisonError::into_inner), [Some(CancelReason::Drain)], "and was cancelled by the drain's end");
    gate.open();
    within("execution", working).await.unwrap().expect("the execution was refused");
    drop(app);
}

/// A server whose `drain` opens a terminal execution and keeps it in `slot`.
fn terminal_server(log: &Log, slot: &Arc<Mutex<Option<Execution>>>) -> TestServer<TA> {
    let slot = Arc::clone(slot);
    let drain: OnDrain = Arc::new(move |handle, token| {
        let slot = Arc::clone(&slot);
        Box::pin(async move {
            let exec = Execution::open_terminal(&token, &handle.root(), ExecOptions::new()).expect("a terminal execution opens in the drain");
            *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(exec);
        })
    });
    TestServer::<TA>::new("a", log).on_drain(drain)
}

#[tokio::test]
async fn a_terminal_execution_opened_in_the_drain_is_waited_for_like_any_other() {
    let log = Log::new();
    let slot: Arc<Mutex<Option<Execution>>> = Arc::default();
    let app = listened(
        App::builder(Empty).runtime(Tokio::current()).drain_timeout(Duration::from_secs(30)),
        vec![server(terminal_server(&log, &slot))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));
    let closing = tokio::spawn(handle.close(Signal::new("done")));
    until(&log, "drain a").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(slot.lock().unwrap_or_else(PoisonError::into_inner).is_some(), "the drain opened its terminal execution");
    assert!(!closing.is_finished(), "the drain did not wait for the terminal execution");

    drop(slot.lock().unwrap_or_else(PoisonError::into_inner).take());
    let report = within("close", closing).await.unwrap().expect("shutdown reported a failure");
    assert_eq!((report.abandoned, report.terminal_skipped), (0, 0), "it ended inside the window, so nothing is abandoned or skipped");
    within("serve", serving).await.unwrap().expect("shutdown reported a failure");
}

#[tokio::test]
async fn a_terminal_execution_still_open_at_the_drain_timeout_is_cancelled_and_counted() {
    let log = Log::new();
    let slot: Arc<Mutex<Option<Execution>>> = Arc::default();
    let app = listened(
        App::builder(Empty).runtime(Tokio::current()).drain_timeout(Duration::from_millis(50)),
        vec![server(terminal_server(&log, &slot))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let handle = app.handle();
    let serving = tokio::spawn(app.serve(std::future::pending::<Signal>()));

    let report = within("close", handle.close(Signal::new("done"))).await.expect("shutdown reported a failure");
    assert_eq!((report.abandoned, report.terminal_skipped), (1, 1), "the terminal execution is abandoned and counted as skipped");
    let exec = slot.lock().unwrap_or_else(PoisonError::into_inner).take().expect("the drain opened its terminal execution");
    assert_eq!((exec.is_cancelled(), exec.cancel_reason()), (true, Some(CancelReason::Drain)));
    within("serve", serving).await.unwrap().expect("shutdown reported a failure");
}

#[tokio::test]
async fn a_shutdown_whose_runner_is_dropped_mid_hook_resumes_without_running_a_hook_twice() {
    let log = Log::new();
    let gate = Gate::new(false);
    let (l, g) = (log.clone(), Arc::clone(&gate));
    let app = connects(Mod::<A>::new(move |m: &mut ModuleDef<'_>| {
        let (first, middle, last) = (l.clone(), l.clone(), l.clone());
        let g = Arc::clone(&g);
        m.singleton(|| async { S1 }).before_shutdown(move |_s: Signal| {
            let log = first.clone();
            async move { log.push("h1") }
        });
        m.singleton(|| async { S2 }).before_shutdown(move |_s: Signal| {
            let (log, gate) = (middle.clone(), Arc::clone(&g));
            async move {
                log.push("h2 start");
                gate.pass().await;
                log.push("h2 end");
            }
        });
        m.singleton(|| async { S3 })
            .before_shutdown(move |_s: Signal| {
                let log = last.clone();
                async move { log.push("h3") }
            })
            .on_destroy(log_destroy!(l, "destroy"));
    }))
    .await;
    let handle = app.handle();

    let mut abandoned = Box::pin(handle.close(Signal::new("first")));
    let outcome = tokio::time::timeout(Duration::from_millis(100), abandoned.as_mut()).await;
    assert!(outcome.is_err(), "the shutdown finished though a hook was still waiting");
    assert!(log.has("h2 start"));
    drop(abandoned);

    let report = within("second close", handle.close(Signal::new("second"))).await.expect("shutdown reported a failure");
    assert_eq!(report.signal.name(), "first", "the resumed shutdown keeps the trigger that started it");
    assert_eq!(
        log.events(),
        ["h3", "h2 start", "h1", "destroy"],
        "the hook that was running when its runner dropped is not started again, nor are the ones before it; the rest run once"
    );
    drop(app);
}

/// A server for `TC` whose `serve` fails at once, and which logs its `close` as `close dying`.
struct Dying(Log);

impl ulo::Server for Dying {
    type Transport = TC;

    async fn bind(&mut self, _mounted: ulo::Mounted<'_, TC>) -> Result<(), ulo::BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), ulo::BoxError> {
        Err(Box::new(Boom::new("the accept loop died")))
    }

    async fn drain(&self, _token: ulo::DrainToken) {}

    async fn close(&self) -> Result<(), ulo::BoxError> {
        self.0.push("close dying");
        Ok(())
    }
}

#[tokio::test]
async fn every_shutdown_path_that_reaches_the_close_step_closes_each_server_once() {
    type Serve = fn(App<ulo::app::Bound>) -> ulo::BoxFuture<'static, Result<ulo::Shutdown, ulo::ShutdownError>>;
    let on_signal: Serve = |app| Box::pin(app.serve(async { Signal::new("signal") }));
    let on_handle_close: Serve = |app| {
        let handle = app.handle();
        Box::pin(async move {
            let serving = tokio::spawn(app.serve(std::future::pending()));
            let _ = handle.close(Signal::new("handle")).await;
            serving.await.expect("serve did not panic")
        })
    };
    let on_serve_death: Serve = |app| Box::pin(app.serve(std::future::pending()));
    let paths: [(&str, Serve, bool, Option<Duration>); 4] = [
        ("signal", on_signal, false, None),
        ("handle close", on_handle_close, false, None),
        ("serve-loop death", on_serve_death, true, None),
        ("expired cap", on_signal, false, Some(Duration::ZERO)),
    ];
    for (path, serve, dying, cap) in paths {
        let log = Log::new();
        let mut builder = frozen(Empty);
        if let Some(cap) = cap {
            builder = builder.shutdown_timeout(cap);
        }
        let mut servers = vec![server(TestServer::<TA>::new("a", &log)), server(TestServer::<TB>::new("b", &log))];
        if dying {
            servers.push(server(Dying(log.clone())));
        }
        let app = listened(builder, servers).await.unwrap_or_else(|error| panic!("{path}: listen() failed: {error}"));
        let handle = app.handle();
        let outcome = within(path, serve(app)).await;
        assert_eq!(outcome.is_err(), dying, "{path}: only the serve-loop death is a failure: {outcome:?}");
        // A later `close` answers the same outcome and closes nothing again.
        let again = within(path, handle.close(Signal::new("again"))).await;
        assert_eq!(again.is_err(), dying, "{path}");
        for name in ["close a", "close b"] {
            assert_eq!(log.count(name), 1, "{path}: `{name}` ran {} times: {:?}", log.count(name), log.events());
        }
        assert_eq!(log.count("close dying"), usize::from(dying), "{path}: the dead server's `close` runs too: {:?}", log.events());
    }
}

/// The closes all start in the close step's first poll, so a runner dropped during the step
/// leaves its successor nothing to start: each `close` runs once.
#[tokio::test]
async fn a_shutdown_whose_runner_is_dropped_mid_close_step_starts_no_close_again() {
    let log = Log::new();
    let app = listened(
        App::builder(Empty).runtime(Tokio::current()).drain_timeout(Duration::from_millis(10)),
        vec![
            server(TestServer::<TA>::new("a", &log)),
            server(TestServer::<TB>::new("b", &log).close_with(CloseMode::Pending)),
        ],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let handle = app.handle();
    let closes = || log.events().into_iter().filter(|e| e.starts_with("close")).collect::<Vec<String>>();

    let mut abandoned = Box::pin(handle.close(Signal::new("first")));
    let outcome = tokio::time::timeout(Duration::from_millis(300), abandoned.as_mut()).await;
    assert!(outcome.is_err(), "the shutdown finished though `b`'s close never ends");
    assert_eq!(closes(), ["close b", "close a"], "both closes started in the step's first poll");
    drop(abandoned);

    let _ = within("second close", handle.close(Signal::new("second"))).await;
    assert_eq!(closes(), ["close b", "close a"], "the resumed shutdown started a close again");
    drop(app);
}

/// A server for `TA` whose `close` holds its first poll for `0` on the real clock, as a `close`
/// polling many connections once does, then ends.
struct SlowFirstPoll(Duration);

impl ulo::Server for SlowFirstPoll {
    type Transport = TA;

    async fn bind(&mut self, _mounted: ulo::Mounted<'_, TA>) -> Result<(), ulo::BoxError> {
        Ok(())
    }

    async fn serve(&self) -> Result<(), ulo::BoxError> {
        std::future::pending().await
    }

    async fn drain(&self, _token: ulo::DrainToken) {}

    async fn close(&self) -> Result<(), ulo::BoxError> {
        std::thread::sleep(self.0);
        Ok(())
    }
}

/// The cap ends at one instant counted from the trigger: a drain that took part of it leaves the
/// closes the rest, and a `close` first polled after another's slow first poll is cut at that same
/// instant, not given the remainder again from its own start. 300 ms of drain and 200 ms of `a`'s
/// first poll leave `b` 200 ms of a 700 ms cap.
#[tokio::test]
async fn a_close_is_cut_where_the_cap_ends_whatever_ran_before_it() {
    let log = Log::new();
    let slow_drain: OnDrain = Arc::new(|_, _| Box::pin(tokio::time::sleep(Duration::from_millis(300))));
    let app = listened(
        App::builder(Empty).runtime(Tokio::current()).drain_timeout(Duration::from_secs(5)).shutdown_timeout(Duration::from_millis(700)),
        vec![
            server(TestServer::<TB>::new("b", &log).close_with(CloseMode::Pending).on_drain(slow_drain)),
            server(SlowFirstPoll(Duration::from_millis(200))),
        ],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    let started = Instant::now();
    let error = within("serve", app.serve(async { Signal::new("done") })).await.expect_err("shutdown reported no failure");
    let took = started.elapsed();

    match &error.failures[..] {
        [ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } }] => {
            assert_eq!(*transport, TypeName::of::<TB>());
            assert_eq!((*after, *limit), (Duration::from_millis(700), Limit::ShutdownCap));
        }
        other => panic!("expected `b`'s close cut by the cap alone, got: {other:?}"),
    }
    assert!(took >= Duration::from_millis(650), "the close was cut before the cap ended: {took:?}");
    assert!(took < Duration::from_millis(850), "the shutdown outlived its 700 ms cap: {took:?}");
}
