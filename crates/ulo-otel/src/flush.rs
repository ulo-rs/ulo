use std::thread;
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
    let (done, finished) = oneshot::channel();
    let spawned = thread::Builder::new().name(String::from("ulo-otel.flush")).spawn(move || {
        let outcome = provider.shutdown_with_timeout(timeout).map_err(BoxError::from);
        // The hook has stopped listening once its bound expired; the outcome then has no reader.
        let _ = done.send(outcome);
    });
    match spawned {
        Ok(_detached) => finished,
        Err(error) => {
            // The closure, and the sender inside it, is dropped with the error, so the receiver
            // above would answer only `Canceled`; this one carries the reason.
            let (done, finished) = oneshot::channel();
            let _ = done.send(Err(format!("the span flush thread could not be started: {error}").into()));
            finished
        }
    }
}
