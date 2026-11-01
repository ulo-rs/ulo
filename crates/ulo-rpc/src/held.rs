//! A broker link's held `cancel`, and, behind `conformance-probe`, the probe the RPC conformance
//! suite sees and shortens the hold through.
//!
//! NATS, RabbitMQ, MQTT and Kafka hold a streamed request's `cancel` sent before the server
//! acknowledged its `open`, and send it alone when the `opened` arrives. A held `cancel` is kept
//! until the deadline its call's `open` carried in `deadline-ms`, past which the server ends the
//! call itself, or for [`CANCEL_HOLD`] after the `cancel` when the `open` carried none, and then
//! dropped with its entry, so a call whose `opened` never comes holds nothing until the link
//! closes.

#[cfg(feature = "conformance-probe")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "conformance-probe")]
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::transport::CallHeaders;

/// How long a broker link keeps a `cancel` waiting for its call's `opened` when the call's `open`
/// carried no `deadline-ms`: the RPC client's default timeout, so an `opened` later than a whole
/// default wait after the `cancel` is taken as one that never comes.
pub const CANCEL_HOLD: Duration = Duration::from_secs(5);

/// The deadline a streamed request's `open` carried, fixed when the link sends the `open`, which
/// bounds how long the link keeps the call's held `cancel`.
#[derive(Clone, Copy, Debug)]
pub struct HoldUntil(Option<Instant>);

impl HoldUntil {
    /// Read from the `open`'s headers as the link sends it: `deadline-ms` from now, or none. A
    /// value the server cannot read holds as none does, for the fallback: the server refuses that
    /// call `bad_deadline` before its handler runs, so no deadline of its ends it.
    pub fn at_open(headers: &CallHeaders) -> HoldUntil {
        HoldUntil(headers.deadline().ok().flatten().map(|remaining| Instant::now() + remaining))
    }

    /// How much longer to keep the held `cancel`: until the deadline, or `fallback` for an `open`
    /// that carried none.
    pub fn remaining(&self, fallback: Duration) -> Duration {
        match self.0 {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => fallback,
        }
    }
}

/// One broker link's client side as the RPC conformance suite reaches it: the calls its table
/// holds, the `opened` acknowledgments it withholds on request and delivers late, and the hold
/// on a held `cancel` whose `open` carried no deadline, which a scenario shortens. A link built
/// with its `conformance-probe` feature holds one from its construction and attaches each client
/// side it connects.
#[cfg(feature = "conformance-probe")]
#[derive(Default)]
pub struct ClientProbe {
    withholding: AtomicBool,
    withheld: Mutex<Vec<u64>>,
    hold: Mutex<Option<Duration>>,
    attached: Mutex<Option<Attached>>,
}

#[cfg(feature = "conformance-probe")]
struct Attached {
    opened: Box<dyn Fn(u64) + Send + Sync>,
    calls: Box<dyn Fn() -> usize + Send + Sync>,
}

#[cfg(feature = "conformance-probe")]
impl ClientProbe {
    /// Attaches a connected client side: `opened` takes an acknowledgment for call `id` as if it
    /// had arrived now, and `calls` counts the entries in its table. Replaces the side attached
    /// before.
    pub fn attach(&self, opened: impl Fn(u64) + Send + Sync + 'static, calls: impl Fn() -> usize + Send + Sync + 'static) {
        *lock(&self.attached) = Some(Attached { opened: Box::new(opened), calls: Box::new(calls) });
    }

    /// What the link asks of each `opened` it receives: `true` when the probe withholds it, the
    /// link then dropping it, to be delivered by [`release`](Self::release).
    pub fn withholds(&self, id: u64) -> bool {
        if !self.withholding.load(Ordering::Acquire) {
            return false;
        }
        lock(&self.withheld).push(id);
        true
    }

    /// Withholds every `opened` from now until [`release`](Self::release).
    pub fn withhold(&self) {
        self.withholding.store(true, Ordering::Release);
    }

    /// How many `opened` acknowledgments the probe withholds.
    pub fn withheld(&self) -> usize {
        lock(&self.withheld).len()
    }

    /// Stops withholding and delivers each withheld `opened` to the attached client side.
    pub fn release(&self) {
        self.withholding.store(false, Ordering::Release);
        let withheld = std::mem::take(&mut *lock(&self.withheld));
        if let Some(attached) = &*lock(&self.attached) {
            for id in withheld {
                (attached.opened)(id);
            }
        }
    }

    /// The entries in the attached client side's table, `None` before one is attached.
    pub fn calls(&self) -> Option<usize> {
        lock(&self.attached).as_ref().map(|attached| (attached.calls)())
    }

    /// Keeps a held `cancel` whose `open` carried no deadline for `hold` in place of
    /// [`CANCEL_HOLD`].
    pub fn set_hold(&self, hold: Duration) {
        *lock(&self.hold) = Some(hold);
    }

    /// How long the link keeps a held `cancel` whose `open` carried no deadline.
    pub fn hold(&self) -> Duration {
        lock(&self.hold).unwrap_or(CANCEL_HOLD)
    }
}

#[cfg(feature = "conformance-probe")]
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
