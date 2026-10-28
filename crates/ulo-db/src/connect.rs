use std::error::Error;
use std::future::{Future, poll_fn};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use ulo::{BoxError, BoxFuture, Cut, Permanent, Runtime, RuntimeShutDown, Spawn, TaskEnd, Timer, ValueHandle, spawn_with};

use crate::error::{TaskFailed, UnsupportedRuntime};
use crate::store::Store;

/// The retry policy of one module.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Policy {
    /// The total connect bound.
    pub(crate) connect_within: Duration,
    pub(crate) retries: u32,
    pub(crate) backoff: Duration,
}

/// How deep `carries_permanent` walks a `source()` chain, as the core bounds its own walk.
const MAX_DEPTH: usize = 32;

/// Compares the given runtime's `Spawn::name` with `"tokio"` before anything is built. A comparison
/// and no ambient lookup, so a smol app driven from a thread with a tokio context still answers
/// `"smol"`. `"unknown"` is refused like any other name.
pub(crate) fn require_tokio(runtime: &dyn Runtime, module: &'static str) -> Result<(), UnsupportedRuntime> {
    match Spawn::name(runtime) {
        "tokio" => Ok(()),
        given => Err(UnsupportedRuntime::new(module, "tokio", given)),
    }
}

/// The pool's factory: the name check, then `S::create` through `spawn_with`.
///
/// For options whose `create` connects, the loop around `create` is the store's one retry loop:
/// the module's retries and backoff, ending on `Permanent` or on a backoff the runtime refuses,
/// under the construction bound `connect_within`. For any other options a single `create` runs and `.ready` retries the ping.
///
/// A runtime that refuses the backoff, or the bound, having shut down, fails the loop with a
/// `Permanent` [`RuntimeShutDown`]: its source is the last attempt's error after a refused backoff,
/// the runtime's `Refused` after a refused bound. The bound is a [`ulo::timeout`], so a refusal that
/// arrives once it has passed reads as the bound passing.
pub(crate) async fn create<S: Store>(
    runtime: Arc<dyn Runtime>,
    options: S::Options,
    policy: Policy,
) -> Result<S::Client, BoxError> {
    require_tokio(&*runtime, S::NAME).map_err(|e| BoxError::from(Permanent::new(e)))?;
    if !S::create_connects(&options) {
        return create_once::<S>(&runtime, &options).await;
    }
    let looping = create_looping::<S>(&runtime, &options, policy);
    match ulo::timeout(&*runtime, policy.connect_within, looping).await {
        Ok(created) => created,
        Err(Cut::Refused(refused)) => Err(Permanent::new(RuntimeShutDown::new(refused)).into()),
        // The binding's construction bound is the same length on the same clock and started no
        // later than this one, so it has passed too, and it reports the expiry when it drops this
        // future, in the poll this one ended in.
        Err(Cut::Expired) => std::future::pending().await,
    }
}

/// The retries of [`create`], dropped at the connect bound, which aborts the attempt in flight.
async fn create_looping<S: Store>(runtime: &Arc<dyn Runtime>, options: &S::Options, policy: Policy) -> Result<S::Client, BoxError> {
    let mut made: u32 = 0;
    loop {
        made = made.saturating_add(1);
        let error = match create_once::<S>(runtime, options).await {
            Ok(client) => return Ok(client),
            Err(error) => error,
        };
        if carries_permanent(&*error) || error.is::<TaskFailed>() || made > policy.retries {
            return Err(error);
        }
        // The error's text is left out: it can carry the URL, and nothing scrubs a log line.
        tracing::debug!(store = S::NAME, attempt = made, "creating the client failed; retrying");
        if !policy.backoff.is_zero()
            && let Err(refused) = Timer::try_sleep(&**runtime, policy.backoff).await
        {
            // On `sleep`, which ends at once on a runtime that has shut down, the retries would
            // run without pause.
            tracing::debug!(store = S::NAME, %refused, "the runtime refused the backoff before a retry of `create`; it is not retried");
            return Err(Permanent::new(RuntimeShutDown::after(error)).into());
        }
    }
}

/// One `S::create`, called inside a task on the given runtime so that whatever the library spawns
/// while building the client is spawned there.
async fn create_once<S: Store>(runtime: &Arc<dyn Runtime>, options: &S::Options) -> Result<S::Client, BoxError> {
    let options = options.clone();
    let attempt = Attached(spawn_with(&**runtime, async move { S::create(&options).await }));
    match attempt.await {
        Ok(created) => created,
        Err(end) => Err(TaskFailed::new(S::NAME, "create", end).into()),
    }
}

/// `S::ping` through `spawn_with`.
pub(crate) fn ping<S: Store>(runtime: Arc<dyn Runtime>, client: S::Client) -> BoxFuture<'static, Result<(), BoxError>> {
    Box::pin(async move {
        let attempt = Attached(spawn_with(&*runtime, async move { S::ping(&client).await }));
        match attempt.await {
            Ok(pinged) => pinged,
            Err(end) => Err(TaskFailed::new(S::NAME, "ping", end).into()),
        }
    })
}

/// `S::close` through `spawn_with`.
pub(crate) fn close<S: Store>(runtime: Arc<dyn Runtime>, client: S::Client) -> BoxFuture<'static, ()> {
    Box::pin(async move {
        let closing = Attached(spawn_with(&*runtime, async move { S::close(client).await }));
        if let Err(end) = closing.await {
            let failed = TaskFailed::new(S::NAME, "close", end);
            tracing::warn!(store = S::NAME, error = %failed, "closing the client did not finish");
        }
    })
}

/// Whether `error` or anything down its `source()` chain is a `Permanent`, as the core's readiness
/// loop reads an attempt's error.
pub(crate) fn carries_permanent(error: &(dyn Error + 'static)) -> bool {
    let mut next = Some(error);
    for _ in 0..MAX_DEPTH {
        match next {
            Some(e) if e.is::<Permanent>() => return true,
            Some(e) => next = e.source(),
            None => return false,
        }
    }
    false
}

/// A spawned task that is aborted when its handle is dropped, where `ValueHandle` alone detaches it.
/// The core drops a construction, a readiness attempt or a hook at its bound, and the health run
/// drops a check at its own; the task doing that work stops with it rather than running on unseen.
pub(crate) struct Attached<T>(pub(crate) ValueHandle<T>);

impl<T> Future for Attached<T> {
    type Output = Result<T, TaskEnd>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

/// Aborting a task that has ended does nothing.
impl<T> Drop for Attached<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// `work`'s output, or `None` once `bound` passes on the app's clock first. `work` is dropped with
/// the returned future either way. For a bound whose expiry only ends work: a runtime that has shut
/// down ends the bound at once, which reads as the bound passing.
pub(crate) fn within<F: Future + Unpin>(
    runtime: &dyn Runtime,
    bound: Duration,
    mut work: F,
) -> impl Future<Output = Option<F::Output>> + use<F> {
    let mut expiry = Timer::sleep(runtime, bound);
    poll_fn(move |cx: &mut Context<'_>| {
        if let Poll::Ready(output) = Pin::new(&mut work).poll(cx) {
            return Poll::Ready(Some(output));
        }
        if expiry.as_mut().poll(cx).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
}
