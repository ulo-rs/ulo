//! An explicit `.timer(..)` is the app's clock in any order with `.runtime(..)`; a runtime
//! supplies the clock only when none was given, and the last runtime given supplies it.

use std::task::{Context, Waker};
use std::time::{Duration, Instant};

use ulo::testing::{ScriptedTimer, TestApp};
use ulo::{
    App, AppBuilder, AppHandle, BoxFuture, Module, ModuleDef, ModuleIdentity, Refused, Spawn,
    TaskHandle, Timer,
};
use ulo_runtime_tokio::Tokio;

struct Empty;

impl Module for Empty {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// A runtime that spawns on tokio and times with a scripted clock of its own.
struct Scripted(ScriptedTimer);

impl Timer for Scripted {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.0.sleep(d)
    }

    fn now(&self) -> Instant {
        self.0.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        self.0.try_sleep(d)
    }
}

impl Spawn for Scripted {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        Tokio::current().spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        Tokio::current().try_spawn(fut)
    }

    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>) -> TaskHandle {
        Tokio::current().spawn_blocking(work)
    }
}

const GRACE: Duration = Duration::from_secs(30);

/// Whether a sleep through `timer`, polled once, registers on `clock`.
fn times_on(timer: &dyn Timer, clock: &ScriptedTimer) -> bool {
    let mut sleep = timer.sleep(GRACE);
    assert!(
        sleep
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
    let registered = clock.live(GRACE) == 1;
    drop(sleep);
    registered
}

/// `AppHandle::timer` and `AppHandle::runtime` both time on `clock`.
fn assert_clock(handle: &AppHandle, clock: &ScriptedTimer) {
    assert!(
        times_on(&**handle.timer().expect("a clock"), clock),
        "`AppHandle::timer`"
    );
    assert!(
        times_on(&**handle.runtime().expect("a runtime"), clock),
        "`AppHandle::runtime`"
    );
}

async fn built(builder: AppBuilder) -> AppHandle {
    builder
        .wire()
        .expect("wires")
        .connect()
        .await
        .expect("connects")
        .handle()
}

#[tokio::test]
async fn a_timer_before_a_runtime_is_the_clock() {
    let (timer, theirs) = (ScriptedTimer::new(), ScriptedTimer::new());
    assert_clock(
        &built(
            App::builder(Empty)
                .timer(timer.clone())
                .runtime(Scripted(theirs.clone())),
        )
        .await,
        &timer,
    );
    assert_eq!(theirs.live(GRACE), 0);
}

#[tokio::test]
async fn a_timer_after_a_runtime_is_the_clock() {
    let (timer, theirs) = (ScriptedTimer::new(), ScriptedTimer::new());
    assert_clock(
        &built(
            App::builder(Empty)
                .runtime(Scripted(theirs))
                .timer(timer.clone()),
        )
        .await,
        &timer,
    );
}

#[tokio::test]
async fn a_runtime_alone_supplies_its_clock() {
    let theirs = ScriptedTimer::new();
    assert_clock(
        &built(App::builder(Empty).runtime(Scripted(theirs.clone()))).await,
        &theirs,
    );
}

#[tokio::test]
async fn a_second_runtime_supplies_the_clock_when_no_timer_was_given() {
    let (first, second) = (ScriptedTimer::new(), ScriptedTimer::new());
    assert_clock(
        &built(
            App::builder(Empty)
                .runtime(Scripted(first))
                .runtime(Scripted(second.clone())),
        )
        .await,
        &second,
    );
}

#[tokio::test]
async fn a_timer_survives_every_runtime_around_it() {
    let (timer, first, second) = (
        ScriptedTimer::new(),
        ScriptedTimer::new(),
        ScriptedTimer::new(),
    );
    let builder = App::builder(Empty)
        .runtime(Scripted(first))
        .timer(timer.clone())
        .runtime(Scripted(second));
    assert_clock(&built(builder).await, &timer);
}

#[tokio::test]
async fn the_last_timer_wins() {
    let (a, b) = (ScriptedTimer::new(), ScriptedTimer::new());
    let builder = App::builder(Empty)
        .timer(a)
        .runtime(Tokio::current())
        .timer(b.clone());
    assert_clock(&built(builder).await, &b);
}

#[tokio::test]
async fn a_test_app_keeps_its_timer_in_either_order() {
    let (timer, theirs) = (ScriptedTimer::new(), ScriptedTimer::new());
    let before = TestApp::of(Empty)
        .timer(timer.clone())
        .runtime(Scripted(theirs.clone()))
        .connect()
        .await
        .expect("connects");
    assert_clock(&before.handle(), &timer);
    let after = TestApp::of(Empty)
        .runtime(Scripted(theirs))
        .timer(timer.clone())
        .connect()
        .await
        .expect("connects");
    assert_clock(&after.handle(), &timer);
}

#[tokio::test]
async fn a_test_app_runtime_alone_supplies_its_clock() {
    let theirs = ScriptedTimer::new();
    let app = TestApp::of(Empty)
        .runtime(Scripted(theirs.clone()))
        .connect()
        .await
        .expect("connects");
    assert_clock(&app.handle(), &theirs);
}
