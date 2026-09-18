//! Holding an execution open for as long as its answer is still arriving.

use std::pin::Pin;
use std::task::{Context, Poll};

use crate::context::ExecutionContext;

/// A stream that keeps its execution alive, and cancels it if the caller leaves first.
///
/// A handler that answers with a stream returns the moment it has one, so the execution would
/// otherwise end while the items are still being produced (ADR-0016, ADR-0021). Holding the
/// context here carries it to the last item: the cache and the cancellation token stay reachable
/// to whatever is feeding the stream.
///
/// A drop before the stream answers `None` is the caller having gone — a closed connection, a
/// cancel notice, a drain deadline — and the token fires so the producer stops. An item carrying
/// an error is not the end: the transport stops the drain there and drops this un-drained, so the
/// producer behind an abnormal end hears the token too. Nothing else observes any of it, because
/// the handler returned when it had a stream and whatever feeds that stream is not inside the
/// future the adapter drops.
///
/// `S` is held as given rather than pinned here. RPC and WebSocket hand over a `BoxStream`, which
/// is already a `Pin<Box<_>>`; gRPCs is the caller's own stream type with no `Unpin` bound, and
/// `ScopedGrpcStream` names the instantiation that pins it.
pub struct ScopedStream<S, C: ExecutionContext> {
    inner: S,
    context: C,
    drained: bool,
}

impl<S, C: ExecutionContext> ScopedStream<S, C> {
    pub fn new(inner: S, context: C) -> Self {
        Self {
            inner,
            context,
            drained: false,
        }
    }
}

impl<S: futures::Stream + Unpin, C: ExecutionContext + Unpin> futures::Stream
    for ScopedStream<S, C>
{
    type Item = S::Item;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_next(cx);
        if matches!(polled, Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<S, C: ExecutionContext> Drop for ScopedStream<S, C> {
    fn drop(&mut self) {
        if !self.drained {
            self.context.cancellation().cancel();
        }
    }
}
