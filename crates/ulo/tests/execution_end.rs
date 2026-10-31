//! The end of an execution (section 3.8): it ends when its `Execution` drops, whatever
//! `ExecutionRef`s remain. The execution-scoped instances drop then, an instance holding its own
//! `ExecutionRef` among them, so the drain has nothing left to wait for; a handle kept past the end
//! still answers what the execution recorded, and its `get` answers `ExecutionEnded`.

#[macro_use]
mod common;

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use common::*;
use ulo::{App, CancelReason, Dep, ExecOptions, Execution, ExecutionRef, Key, LookupError, Signal};
use ulo::testing::ScriptedTimer;

const DRAIN: Duration = Duration::from_secs(10);

struct A;

/// Counts the `Audit`s built and dropped.
#[derive(Clone, Default)]
struct Counts {
    built: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
}

impl Counts {
    fn built(&self) -> usize {
        self.built.load(Ordering::SeqCst)
    }

    fn dropped(&self) -> usize {
        self.dropped.load(Ordering::SeqCst)
    }
}

/// Section 5's `AuditContext` in the value-level API: execution-scoped and holding its own
/// execution.
struct Audit {
    exec: ExecutionRef,
    counts: Counts,
}

impl Drop for Audit {
    fn drop(&mut self) {
        self.counts.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

struct Ledger;

fn audit_module(counts: &Counts) -> Mod<A> {
    let counts = counts.clone();
    Mod::new(move |m| {
        let counts = counts.clone();
        m.execution(move |exec: ExecutionRef| {
            let counts = counts.clone();
            async move {
                counts.built.fetch_add(1, Ordering::SeqCst);
                Audit { exec, counts }
            }
        });
        m.singleton(|| async { Ledger });
    })
}

async fn on_clock(counts: &Counts, clock: &ScriptedTimer) -> App<ulo::Connected> {
    connected_with(App::builder(audit_module(counts)).timer(clock.clone()).drain_timeout(DRAIN)).await
}

/// Polls `fut` to its end, passing the drain window on `clock` whenever the drain waits on it, so
/// a drain that waits fails the assertion on `clock.elapsed()` rather than hanging the test.
async fn passing_the_window<T>(clock: &ScriptedTimer, fut: impl Future<Output = T>) -> T {
    let mut fut = std::pin::pin!(fut);
    let driven = async {
        loop {
            tokio::select! {
                out = &mut fut => return out,
                () = tokio::time::sleep(Duration::from_millis(1)) => {
                    if clock.live(DRAIN) > 0 {
                        clock.advance(DRAIN);
                    }
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), driven).await.expect("the future ended within ten seconds of real time")
}

#[tokio::test]
async fn an_instance_holding_its_execution_drops_with_the_job_and_close_waits_out_no_window() {
    let counts = Counts::default();
    let clock = ScriptedTimer::new();
    let app = on_clock(&counts, &clock).await;

    for n in 1..=3 {
        let audit = app.execute(ExecOptions::new(), async |exec| exec.get::<Audit>().await.map(|audit| !audit.exec.is_cancelled())).await;
        assert!(audit.unwrap().unwrap(), "the instance reads its own execution while it lasts");
        assert_eq!((counts.built(), counts.dropped()), (n, n), "execution {n}'s instance dropped as `execute` returned");
    }

    let shutdown = passing_the_window(&clock, app.close(Signal::new("done"))).await.unwrap();
    assert_eq!((shutdown.abandoned, shutdown.terminal_skipped), (0, 0));
    assert_eq!(clock.elapsed(), Duration::ZERO, "the drain found nothing alive and passed no window");
}

#[tokio::test]
async fn a_detached_subtasks_handle_answers_what_the_execution_recorded_and_its_get_answers_execution_ended() {
    let counts = Counts::default();
    let clock = ScriptedTimer::new();
    let app = on_clock(&counts, &clock).await;
    let at = Instant::now() + Duration::from_secs(60);
    let go = Arc::new(AtomicBool::new(false));

    let mut subtask = None;
    app.execute(ExecOptions::new().deadline(at), async |exec| {
        exec.extensions().insert(7u32);
        exec.get::<Audit>().await.unwrap();
        let handle = exec.handle();
        let go = Arc::clone(&go);
        subtask = Some(tokio::spawn(async move {
            while !go.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
            let audit = handle.get::<Audit>().await.map(|_| ());
            let singleton = handle.get::<Ledger>().await.map(|_| ());
            let through_resolver = handle.resolver().dep::<Audit>().await.map(|_| ());
            let singleton_through_resolver = handle.resolver().dep::<Ledger>().await.map(|_| ());
            (handle, audit, singleton, through_resolver, singleton_through_resolver)
        }));
    })
    .await
    .unwrap();
    assert_eq!(counts.dropped(), 1, "the execution ended as `execute` returned, the subtask's handle notwithstanding");
    go.store(true, Ordering::SeqCst);
    let (handle, audit, singleton, through_resolver, singleton_through_resolver) = subtask.expect("the job spawned its subtask").await.unwrap();

    match &audit {
        Err(error @ LookupError::ExecutionEnded { key }) => {
            assert_eq!(key.key(), Key::of::<Audit, ()>());
            assert!(error.to_string().contains("execution that has ended"), "{error}");
        }
        other => panic!("expected ExecutionEnded for `Audit`, found {other:?}"),
    }
    assert!(matches!(singleton, Err(LookupError::ExecutionEnded { .. })), "whatever the key's scope: {singleton:?}");
    assert!(
        matches!(&through_resolver, Err(LookupError::ExecutionEnded { key }) if key.key() == Key::of::<Audit, ()>()),
        "the cache refuses a read that reaches it, naming the key: {through_resolver:?}"
    );
    assert!(
        matches!(singleton_through_resolver, Err(LookupError::ExecutionEnded { .. })),
        "the resolver refuses as `get` does: {singleton_through_resolver:?}"
    );
    assert_eq!(counts.built(), 1, "no second instance was built for an execution that is over");

    assert_eq!(handle.deadline(), Some(at));
    assert_eq!(handle.extensions().get::<u32>().as_deref(), Some(&7));
    assert!(!handle.is_cancelled() && handle.cancel_reason().is_none(), "the end fires no cancellation");
    handle.cancel();
    assert_eq!(handle.cancel_reason(), Some(CancelReason::Explicit), "the handle's fields stay usable");

    let shutdown = passing_the_window(&clock, app.close(Signal::new("done"))).await.unwrap();
    assert_eq!(shutdown.abandoned, 0, "a handle kept past the end is not a live execution");
    assert_eq!(clock.elapsed(), Duration::ZERO);
}

#[tokio::test]
async fn an_open_execution_its_instance_awaiting_cancellation_is_drained_and_then_cancelled() {
    let counts = Counts::default();
    let clock = ScriptedTimer::new();
    let app = on_clock(&counts, &clock).await;
    let exec = Execution::open(&app.handle().root(), ExecOptions::new()).unwrap();
    let audit = exec.get::<Audit>().await.unwrap();
    let waiting = tokio::spawn({
        let audit = Dep::clone(&audit);
        async move {
            audit.exec.cancelled().await;
            audit.exec.cancel_reason()
        }
    });
    drop(audit);

    let closing = tokio::spawn(app.close(Signal::new("done")));
    let windowed = async {
        while clock.live(DRAIN) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), windowed).await.expect("the drain waits on its window");
    assert!(!waiting.is_finished(), "nothing cancels the open execution before the drain's end");
    clock.advance(DRAIN);
    let reason = tokio::time::timeout(Duration::from_secs(10), waiting).await.expect("the drain's end cancels it").unwrap();
    assert_eq!(reason, Some(CancelReason::Drain), "the instance's own handle observes the cancellation");
    let shutdown = tokio::time::timeout(Duration::from_secs(10), closing).await.unwrap().unwrap().unwrap();
    assert_eq!(shutdown.abandoned, 1, "the execution open across the window is abandoned");
    assert_eq!(counts.dropped(), 0, "an abandoned execution keeps its instances while its holder lives");
    drop(exec);
    assert_eq!(counts.dropped(), 1, "and they drop with it");
}

#[tokio::test]
async fn a_handles_owned_futures_outlive_the_handle_they_came_from() {
    let app = connected(Empty).await;
    let exec = Execution::open(&app.handle().root(), ExecOptions::new()).unwrap();
    let handle = exec.handle();
    let (cancelled, draining) = (handle.cancelled(), handle.draining());
    drop(handle);
    let cancelled = tokio::spawn(cancelled);
    let draining = tokio::spawn(draining);
    exec.cancel();
    tokio::time::timeout(Duration::from_secs(5), cancelled).await.expect("cancelled() resolves").unwrap();
    drop(exec);
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    assert!(!draining.is_finished(), "draining() waits for the drain, not the execution's cancellation");
    close(app).await;
    tokio::time::timeout(Duration::from_secs(5), draining).await.expect("draining() resolves").unwrap();
}

/// Execution-scoped; its drop waits until the test lets it go, which holds the `Execution`'s drop
/// between releasing the cache and leaving the live set.
struct Lingering {
    dropping: std::sync::mpsc::Sender<()>,
    go: Arc<std::sync::Barrier>,
}

impl Drop for Lingering {
    fn drop(&mut self) {
        let _ = self.dropping.send(());
        self.go.wait();
    }
}

#[tokio::test]
async fn an_execution_whose_drop_is_under_way_at_the_drains_end_is_not_counted_abandoned() {
    let (dropping, dropped) = std::sync::mpsc::channel();
    let go = Arc::new(std::sync::Barrier::new(2));
    let lingering = {
        let go = Arc::clone(&go);
        Mod::<A>::new(move |m| {
            let (dropping, go) = (dropping.clone(), Arc::clone(&go));
            m.execution(move || {
                let (dropping, go) = (dropping.clone(), Arc::clone(&go));
                async move { Lingering { dropping, go } }
            });
        })
    };
    let clock = ScriptedTimer::new();
    let app = connected_with(App::builder(lingering).timer(clock.clone()).drain_timeout(DRAIN)).await;
    let exec = Execution::open(&app.handle().root(), ExecOptions::new()).unwrap();
    drop(exec.get::<Lingering>().await.unwrap());
    let ending = std::thread::spawn(move || drop(exec));
    tokio::task::spawn_blocking(move || dropped.recv()).await.unwrap().expect("the instance began dropping");

    let closing = tokio::spawn(app.close(Signal::new("done")));
    let windowed = async {
        while clock.live(DRAIN) == 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), windowed).await.expect("the drain waits on the entry its drop has not yet left");
    clock.advance(DRAIN);
    let shutdown = tokio::time::timeout(Duration::from_secs(10), closing).await.expect("close returns at the drain's end").unwrap().unwrap();
    tokio::task::spawn_blocking(move || {
        go.wait();
        ending.join()
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(shutdown.abandoned, 0, "an execution whose `Execution` is dropping has ended");
}
