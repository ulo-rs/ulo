//! A scenario's waits and tasks, on the runtime its broker's [`Harness`] names.

use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use futures_util::future::{Either, select};
use ulo::{Runtime, Timer, ValueHandle};

use crate::{Broker, Harness};

/// The scenario's runtime, the one its servers and clients are built on.
pub(crate) fn runtime<B: Broker>() -> Arc<dyn Runtime> {
    Arc::new(<B::Harness as Harness>::runtime())
}

pub(crate) async fn sleep<B: Broker>(d: Duration) {
    <B::Harness as Harness>::runtime().sleep(d).await;
}

/// `fut` as a task of the scenario's runtime, answering its value.
pub(crate) fn spawn<B, T, F>(fut: F) -> ValueHandle<T>
where
    B: Broker,
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    ulo::spawn_with(&<B::Harness as Harness>::runtime(), fut)
}

/// `fut`'s output, or `None` once `within` passes on the scenario's clock.
pub(crate) async fn timeout<B: Broker, T>(within: Duration, fut: impl Future<Output = T>) -> Option<T> {
    let expired = <B::Harness as Harness>::runtime().sleep(within);
    match select(pin!(fut), expired).await {
        Either::Left((output, _)) => Some(output),
        Either::Right(((), _)) => None,
    }
}

/// Lets every other task ready to run go first, once.
pub(crate) async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            return Poll::Ready(());
        }
        yielded = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    })
    .await;
}
