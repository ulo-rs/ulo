//! A clock a test advances by hand.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};
use std::time::{Duration, Instant};

use ulo::{BoxFuture, Timer};

/// The app's clock for a test that pins a deadline without waiting for it: no sleep it hands out
/// ends until [`advance`](Self::advance) has moved the clock past it. Each live sleep is counted
/// by its length, so a test asks whether the code under test is waiting on a deadline of a given
/// length ([`live`](Self::live)) and whether it stopped waiting.
#[derive(Clone)]
pub struct ScriptedTimer(Arc<Mutex<Script>>);

struct Script {
    base: Instant,
    at: Duration,
    live: HashMap<Duration, usize>,
    pending_at: HashMap<Duration, Duration>,
    waiting: Vec<Waker>,
}

struct Sleep {
    clock: ScriptedTimer,
    length: Duration,
    until: Duration,
}

impl ScriptedTimer {
    pub fn new() -> ScriptedTimer {
        ScriptedTimer(Arc::new(Mutex::new(Script { base: Instant::now(), at: Duration::ZERO, live: HashMap::new(), pending_at: HashMap::new(), waiting: Vec::new() })))
    }

    pub fn advance(&self, by: Duration) {
        let waiting = {
            let mut script = self.lock();
            script.at += by;
            std::mem::take(&mut script.waiting)
        };
        waiting.into_iter().for_each(Waker::wake);
    }

    /// The sleeps of exactly `length` that exist and have not been dropped.
    pub fn live(&self, length: Duration) -> usize {
        self.lock().live.get(&length).copied().unwrap_or(0)
    }

    /// The clock time, as [`elapsed`](Self::elapsed) reads it, at which a sleep of exactly
    /// `length` last answered `Pending`; `None` if none has. A value equal to `elapsed()` means the
    /// sleep has seen the latest advance and not ended on it: a test waits on that to know the code
    /// under test observed the clock short of its deadline.
    pub fn pending_at(&self, length: Duration) -> Option<Duration> {
        self.lock().pending_at.get(&length).copied()
    }

    /// How far the clock has been advanced since it was made.
    pub fn elapsed(&self) -> Duration {
        self.lock().at
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Script> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Default for ScriptedTimer {
    fn default() -> Self {
        ScriptedTimer::new()
    }
}

impl Timer for ScriptedTimer {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        let until = {
            let mut script = self.lock();
            *script.live.entry(d).or_default() += 1;
            script.at + d
        };
        Box::pin(Sleep { clock: self.clone(), length: d, until })
    }

    fn now(&self) -> Instant {
        let script = self.lock();
        script.base + script.at
    }
}

impl Future for Sleep {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut script = self.clock.lock();
        if script.at >= self.until {
            return Poll::Ready(());
        }
        let at = script.at;
        script.pending_at.insert(self.length, at);
        script.waiting.push(cx.waker().clone());
        Poll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        if let Some(live) = self.clock.lock().live.get_mut(&self.length) {
            *live -= 1;
        }
    }
}
