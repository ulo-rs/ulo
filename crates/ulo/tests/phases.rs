//! Startup phases and what each shutdown phase allows (section 9.4, 9.5).
//!
//! Capabilities pinned: 43 (build and connect outbound dependencies, then bind sockets, then
//! serve: nothing is prepared or bound before `listen`, and `AppHandle::phase` walks Running,
//! Stopping, Draining, Destroying, Closed as the shutdown sequence runs) and the runtime rows of
//! section 3 for the phase table: new executions, singleton lookups and `load` in each phase, and
//! `open_terminal` allowed during the drain only.
//!
//! The observations are made from where each phase can be seen: a before-shutdown hook (Stopping),
//! a server's `drain` (Draining), a module's destroy and shutdown hooks (Destroying), and after
//! `serve` returns (Closed). `open_terminal` needs the `DrainToken` only a server's `drain`
//! receives, so the token is kept from there.
//!
//! What a terminal execution does to the drain itself (waited for, abandoned, counted) is in
//! `shutdown.rs`.

#[macro_use]
mod common;

use std::sync::{Arc, Mutex, PoisonError};

use common::*;
use ulo::{AppHandle, DrainToken, ExecOptions, Execution, LoadError, LookupError, Phase, Signal};
use ulo_runtime_tokio::Tokio;

struct A;
struct L;
struct S1;

type Slot<T> = Arc<Mutex<Option<T>>>;

fn put<T>(slot: &Slot<T>, value: T) {
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
}

fn take<T: Clone>(slot: &Slot<T>) -> Option<T> {
    slot.lock().unwrap_or_else(PoisonError::into_inner).clone()
}

/// Logs what `handle` allows right now, each line prefixed with `at`: the phase, `is_draining`,
/// whether a new execution opens, whether a singleton lookup answers, whether `load` is refused
/// and, given a token, whether a terminal execution opens.
async fn observe(handle: AppHandle, token: Option<DrainToken>, log: Log, at: &'static str) {
    let say = |what: String| log.push(format!("{at} {what}"));
    say(format!("phase {:?}", handle.phase()));
    say(format!("draining {}", handle.is_draining()));
    say(match handle.execute(ExecOptions::new(), async |_exec| ()).await {
        Ok(()) => "execute ok".into(),
        Err(_) => "execute closed".into(),
    });
    say(match Execution::open(&handle.root(), ExecOptions::new()) {
        Ok(_) => "open ok".into(),
        Err(_) => "open closed".into(),
    });
    say(match handle.get::<S1>().await {
        Ok(_) => "get ok".into(),
        Err(LookupError::Closed { .. }) => "get closed".into(),
        Err(other) => format!("get {other}"),
    });
    say(match handle.load(Mod::<L>::bare()).await {
        Ok(_) => "load ok".into(),
        Err(LoadError::Closed(_)) => "load closed".into(),
        Err(other) => format!("load {other}"),
    });
    if let Some(token) = token {
        say(match Execution::open_terminal(&token, &handle.root(), ExecOptions::new()) {
            Ok(_) => "terminal ok".into(),
            Err(_) => "terminal closed".into(),
        });
    }
}

/// An app with one server, observed at each phase of its shutdown. Returns every line logged and
/// the shutdown report.
async fn observed() -> (Vec<String>, ulo::Shutdown) {
    let log = Log::new();
    let handle: Slot<AppHandle> = Slot::default();
    let token: Slot<DrainToken> = Slot::default();

    let (l, h, t) = (log.clone(), handle.clone(), token.clone());
    let root = Mod::<A>::new(move |m| {
        m.singleton(|| async { S1 });
        let (log, handle) = (l.clone(), h.clone());
        m.before_shutdown(move |_s: Signal| {
            let (log, handle) = (log.clone(), take(&handle).expect("the handle was set"));
            observe(handle, None, log, "stopping")
        });
        let (log, handle, token) = (l.clone(), h.clone(), t.clone());
        m.on_destroy(move || {
            let (log, handle, token) = (log.clone(), take(&handle).expect("the handle was set"), take(&token));
            observe(handle, token, log, "destroying")
        });
        let (log, handle, token) = (l.clone(), h.clone(), t.clone());
        m.on_shutdown(move |_s: Signal| {
            let (log, handle, token) = (log.clone(), take(&handle).expect("the handle was set"), take(&token));
            observe(handle, token, log, "shutdown")
        });
    });

    let (l, t) = (log.clone(), token.clone());
    let drain: OnDrain = Arc::new(move |handle, drain_token| {
        let (log, token) = (l.clone(), t.clone());
        Box::pin(async move {
            put(&token, drain_token.clone());
            observe(handle, Some(drain_token), log, "draining").await;
        })
    });
    let app = listened(
        ulo::App::builder(root).runtime(Tokio::current()),
        vec![server(TestServer::<TA>::new("a", &log).on_drain(drain))],
    )
    .await
    .unwrap_or_else(|error| panic!("listen() failed: {error}"));
    put(&handle, app.handle());
    let handle = app.handle();

    let report = within("serve", app.serve(async { Signal::new("done") })).await.expect("shutdown reported a failure");
    observe(handle, take(&token), log.clone(), "closed").await;
    (log.events(), report)
}

fn at(events: &[String], phase: &str) -> Vec<String> {
    let prefix = format!("{phase} ");
    events.iter().filter_map(|e| e.strip_prefix(&prefix)).map(str::to_owned).collect()
}

#[tokio::test]
async fn nothing_is_prepared_or_bound_before_listen_and_the_phases_walk_to_closed() {
    watchdog();
    let log = Log::new();
    let app = ulo::App::builder(Empty)
        .runtime(Tokio::current())
        .wire()
        .unwrap()
        .connect()
        .await
        .expect("connect failed")
        .bind(TestServer::<TA>::new("a", &log));
    assert_eq!(app.handle().phase(), Phase::Running, "a connected app is running");
    assert!(log.events().is_empty(), "a queued server was touched before `listen`: {:?}", log.events());

    let app = app.listen().await.unwrap_or_else(|error| panic!("listen() failed: {error}"));
    assert_eq!(log.events(), ["prepare a", "bind a"], "`listen` prepares, then binds");
    let handle = app.handle();
    assert_eq!(handle.phase(), Phase::Running, "a bound app is running until a shutdown starts");

    app.serve(async { Signal::new("done") }).await.expect("shutdown reported a failure");
    assert_eq!(handle.phase(), Phase::Closed);
    assert!(handle.is_draining());
}

#[tokio::test]
async fn the_shutdown_phases_are_seen_from_each_step_that_runs_in_them() {
    let (events, report) = observed().await;
    let phases: Vec<String> = ["stopping", "draining", "destroying", "shutdown", "closed"]
        .into_iter()
        .map(|at| at_first(&events, at, "phase"))
        .collect();
    assert_eq!(
        phases,
        ["Stopping", "Draining", "Destroying", "Destroying", "Closed"],
        "before-shutdown runs in Stopping, a server's drain in Draining, destroy and shutdown hooks in Destroying"
    );
    assert_eq!(report.signal.name(), "done");
}

fn at_first(events: &[String], at: &str, what: &str) -> String {
    let prefix = format!("{at} {what} ");
    events
        .iter()
        .find_map(|e| e.strip_prefix(&prefix))
        .unwrap_or_else(|| panic!("`{at}` never logged `{what}`: {events:?}"))
        .to_owned()
}

#[tokio::test]
async fn stopping_still_serves_new_executions_and_lookups_but_refuses_load() {
    let (events, _) = observed().await;
    assert_eq!(
        at(&events, "stopping"),
        ["phase Stopping", "draining false", "execute ok", "open ok", "get ok", "load closed"],
        "traffic is normal through the before-shutdown step, and a module loaded now would miss it"
    );
}

#[tokio::test]
async fn draining_refuses_new_executions_and_load_keeps_lookups_and_allows_a_terminal_one() {
    let (events, _) = observed().await;
    assert_eq!(
        at(&events, "draining"),
        ["phase Draining", "draining true", "execute closed", "open closed", "get ok", "load closed", "terminal ok"],
        "new work is refused from the stop-accepting step on; a connection's cleanup may still open"
    );
}

#[tokio::test]
async fn destroying_onward_refuses_singleton_lookups_and_a_terminal_execution_too() {
    let (events, report) = observed().await;
    let closed_all = ["draining true", "execute closed", "open closed", "get closed", "load closed", "terminal closed"];
    for (at_step, phase) in [("destroying", "Destroying"), ("shutdown", "Destroying"), ("closed", "Closed")] {
        let seen = at(&events, at_step);
        assert_eq!(seen[0], format!("phase {phase}"), "{at_step}: {seen:?}");
        assert_eq!(seen[1..], closed_all, "{at_step}: everything is refused");
    }
    assert_eq!(
        report.terminal_skipped, 2,
        "the terminal executions refused during the destroy and shutdown steps are counted; the one after the shutdown is not part of it"
    );
}
