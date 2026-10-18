use std::time::Duration;

use futures_channel::oneshot;
use opentelemetry_sdk::trace::SdkTracerProvider;
use ulo::BoxError;

/// Runs the batch processor's `shutdown` on a thread of its own and signals its end through the
/// returned oneshot, which the `OnApplicationShutdown` hook awaits under its bound. `shutdown` blocks
/// until the processor's thread has flushed or its timeout passes, and stalls for the whole timeout
/// when the exporter's future needs a runtime the caller is blocking, so it never runs on a worker.
/// On the hook's expiry the thread is left running, detached.
pub(crate) fn spawn(provider: SdkTracerProvider, timeout: Duration) -> oneshot::Receiver<Result<(), BoxError>> {
    let _ = (provider, timeout);
    todo!()
}
