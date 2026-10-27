//! A runtime that has shut down refuses to spawn and to time as a value, never a panic; a running
//! one refuses neither.

use std::future::pending;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Waker};
use std::time::Duration;

use ulo::{Refused, Runtime};

use super::{SETTLE, within};
use crate::Stoppable;

/// Longer than the scenario waits for anything, so a sleep of it that ends ended without its time
/// passing.
const LONG: Duration = Duration::from_secs(3600);

/// `rt`, running, spawns and times through `try_spawn` and `try_sleep`, and the sleep lasts its
/// length on `rt`'s clock.
pub async fn running<R: Runtime>(rt: R) {
    let task = rt.try_spawn(Box::pin(async {})).expect("a running runtime refused a spawn");
    let end = within(&rt, task).await.expect("a task `try_spawn` started did not end");
    assert!(end.is_finished(), "a task `try_spawn` started on a running runtime ended {end:?}");
    let began = rt.now();
    let slept = within(&rt, rt.try_sleep(SETTLE)).await;
    let took = rt.now().saturating_duration_since(began);
    assert_eq!(slept, Some(Ok(())), "a running runtime's `try_sleep` answered {slept:?}");
    assert!(took >= SETTLE, "a running runtime's `try_sleep` of {SETTLE:?} ended after {took:?}");
}

/// Once `stoppable`'s runtime has shut down: `try_spawn` answers `Err(Refused)` with the future
/// unrun, `spawn` a handle that ends `Aborted` with the future unrun, `try_sleep` `Err(Refused)` at
/// once, `sleep` ends at once, a sleep under way at the shutdown answers `Err(Refused)`, a task
/// pending then ends `Aborted`, and `now` answers. Every wait is timed on `rt`.
pub async fn shut_down<R: Runtime>(rt: R, stoppable: Stoppable<R>) {
    let Stoppable { runtime: stopped, stop } = stoppable;
    let pending_task = stopped.try_spawn(Box::pin(pending())).expect("a running runtime refused a spawn ahead of its shutdown");
    let mut sleeping = stopped.try_sleep(LONG);
    assert!(
        sleeping.as_mut().poll(&mut Context::from_waker(Waker::noop())).is_pending(),
        "an hour's `try_sleep` ended at its first poll on a running runtime"
    );
    // A turn for the runtime to take up the task and the sleep before it shuts down.
    rt.sleep(SETTLE).await;
    stop();

    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let refused = stopped.try_spawn(Box::pin(async move { flag.store(true, Ordering::SeqCst) }));
    assert!(refused.is_err(), "a runtime that has shut down answered `try_spawn` with a handle");
    let flag = Arc::clone(&ran);
    let handle = stopped.spawn(Box::pin(async move { flag.store(true, Ordering::SeqCst) }));
    let end = within(&rt, handle).await.expect("the handle `spawn` answered on a runtime that has shut down did not end");
    assert!(end.is_aborted(), "a task spawned on a runtime that has shut down ended {end:?}");
    assert!(!ran.load(Ordering::SeqCst), "a task spawned on a runtime that has shut down ran");

    let slept = within(&rt, stopped.try_sleep(LONG)).await;
    assert_eq!(slept, Some(Err(Refused)), "a runtime that has shut down answered an hour's `try_sleep` with {slept:?}");
    let slept = within(&rt, stopped.sleep(LONG)).await;
    assert!(slept.is_some(), "an hour's `sleep` on a runtime that has shut down did not end at once");
    let slept = within(&rt, sleeping).await;
    assert_eq!(slept, Some(Err(Refused)), "a `try_sleep` under way when its runtime shut down answered {slept:?}");
    let end = within(&rt, pending_task).await.expect("a task pending when its runtime shut down did not end");
    assert!(end.is_aborted(), "a task pending when its runtime shut down ended {end:?}");
    let _ = stopped.now();
}
