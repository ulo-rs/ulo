//! What every transport does with its resolved enhancers once a call arrives.
//!
//! An entry is either an instance shared by every call or a factory asked for one per call, and
//! turning a slice of entries into the instances this call runs is the same work whatever arrived.
//! So is offering an error to one chain handler: a handler that panics forfeits its claim and the
//! chain continues, because losing the rest of the chain would lose the original error too.

use std::sync::Arc;

use crate::enhancer::{Guard, Interceptor};
use crate::errors::PipelineSegment;
use crate::spi::transport::{ErrorHandlerArc, GuardEntry, InterceptorEntry, Transport};

/// The guards this call runs, in declaration order.
pub(crate) async fn guards_for<T: Transport>(
    entries: &[GuardEntry<T>],
    ctx: &T::Context,
) -> Vec<Arc<dyn Guard<T::Context>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        out.push(match entry {
            GuardEntry::Ready(guard) => guard.clone(),
            GuardEntry::Factory(factory) => factory.create(ctx).await,
        });
    }
    out
}

/// The interceptors this call runs, outermost first.
pub(crate) async fn interceptors_for<T: Transport>(
    entries: &[InterceptorEntry<T>],
    ctx: &T::Context,
) -> Vec<Arc<dyn Interceptor<T::Context, T::Answer>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        out.push(match entry {
            InterceptorEntry::Ready(interceptor) => interceptor.clone(),
            InterceptorEntry::Factory(factory) => factory.create(ctx).await,
        });
    }
    out
}

/// Offer one error to one chain handler.
///
/// `None` means the handler declined, and a handler that panics declines too: the panic is logged
/// against the position it came from and the chain moves on, which keeps one bad handler from
/// losing the error for every handler after it.
pub(crate) async fn offer_to<T: Transport>(
    handler: &ErrorHandlerArc<T>,
    error: &(dyn std::error::Error + Send + Sync + 'static),
    ctx: &T::Context,
    position: usize,
) -> Option<T::Answer> {
    match crate::panic_recovery::catch_async(
        PipelineSegment::ErrorHandler,
        handler.handle_error(error, ctx),
    )
    .await
    {
        Ok(claimed) => claimed,
        Err(panic_event) => {
            tracing::error!(
                chain_position = position,
                error = %error,
                panic = %panic_event.message,
                "error handler panicked; trying the next one"
            );
            None
        }
    }
}
