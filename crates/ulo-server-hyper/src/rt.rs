//! hyper's runtime edges over the app's: its executor and timer over a `ulo` `Runtime` and
//! `Timer`.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper::rt::{Executor, Sleep, Timer};
use ulo::{BoxFuture, Runtime};
use ulo_transport::Turns;

/// hyper's executor over a `ulo` [`Runtime`]: each future hyper hands it, HTTP/2's stream and
/// connection tasks among them, is spawned on the runtime and detached. Built from the app's
/// runtime, `Mounted::runtime()` in a server's `bind`, so a connection's tasks run on the runtime
/// the app runs on and a panic in one is redacted with the app's secrets. One built by
/// [`Serve::executor`](crate::Serve::executor) tracks each task in that `Serve`'s turns, so its
/// `close_after` polls an HTTP/2 stream's task as it polls the connections.
#[derive(Clone)]
pub struct RuntimeExecutor {
    runtime: Arc<dyn Runtime>,
    turns: Option<Turns>,
}

impl RuntimeExecutor {
    pub fn new(runtime: Arc<dyn Runtime>) -> RuntimeExecutor {
        RuntimeExecutor { runtime, turns: None }
    }

    pub(crate) fn tracked(runtime: Arc<dyn Runtime>, turns: Turns) -> RuntimeExecutor {
        RuntimeExecutor { runtime, turns: Some(turns) }
    }
}

impl<F> Executor<F> for RuntimeExecutor
where
    F: Future + Send + 'static,
{
    fn execute(&self, fut: F) {
        let fut = async move {
            fut.await;
        };
        // Dropping the handle detaches the task, which hyper's own executors do too.
        match &self.turns {
            Some(turns) => drop(self.runtime.spawn(Box::pin(turns.track(fut)))),
            None => drop(self.runtime.spawn(Box::pin(fut))),
        }
    }
}

/// hyper's timer over a `ulo` [`Timer`](ulo::Timer): the HTTP/1.1 header-read timeout, and any
/// other clock a connection builder reads, on the app's clock. A deadline is measured against that
/// clock's `now`, which hyper reads through [`Timer::now`] too, so a test's fake clock is the one
/// both sides agree on.
#[derive(Clone)]
pub struct RuntimeTimer {
    timer: Arc<dyn ulo::Timer>,
}

impl RuntimeTimer {
    pub fn new(timer: Arc<dyn ulo::Timer>) -> RuntimeTimer {
        RuntimeTimer { timer }
    }
}

impl Timer for RuntimeTimer {
    fn sleep(&self, duration: Duration) -> Pin<Box<dyn Sleep>> {
        Box::pin(AppSleep(Mutex::new(self.timer.sleep(duration))))
    }

    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Sleep>> {
        self.sleep(deadline.saturating_duration_since(self.timer.now()))
    }

    fn now(&self) -> Instant {
        self.timer.now()
    }
}

/// One of the app timer's sleeps as hyper's `Sleep`, which must be `Sync`. The mutex is what makes
/// it so; it is only ever reached through `&mut`, so it is never contended.
struct AppSleep(Mutex<BoxFuture<'static, ()>>);

impl Future for AppSleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        self.get_mut().0.get_mut().unwrap_or_else(PoisonError::into_inner).as_mut().poll(cx)
    }
}

impl Sleep for AppSleep {}
