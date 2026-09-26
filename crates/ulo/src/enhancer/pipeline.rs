//! What every transport does with its resolved enhancers once a call arrives.
//!
//! An entry is either an instance shared by every call or a factory asked for one per call, and
//! turning a slice of entries into the instances this call runs is the same work whatever arrived.
//! So is offering an error to one chain handler: a handler that panics forfeits its claim and the
//! chain continues, because losing the rest of the chain would lose the original error too.

use std::sync::Arc;

use crate::dispatch::transport::{
    Answer, ErrorHandlerArc, GuardEntry, InterceptorEntry, Transport,
};
use crate::enhancer::Interceptor;
use crate::errors::{GuardRejection, PanicRecovered, PipelineSegment};

/// Why [`run_guards`] stopped.
pub(crate) enum GuardFailure {
    /// A guard answered `false`.
    Rejected(GuardRejection),
    /// A guard panicked; `index` is its position in the chain.
    Panicked { index: usize, event: PanicRecovered },
}

/// Build each guard and ask it, one at a time, stopping at the first refusal.
///
/// An entry on the `Factory` arm is an execution-scoped provider's own resolution, with its
/// dependencies constructed with it, so a guard is built only once every guard before it has
/// admitted the call. Nothing below the guards — no interceptor — is built until this returns
/// `Ok`; keeping that is the caller's job.
///
/// A panic in `can_activate` is caught here rather than tearing the call down, and leaves as
/// `Panicked` so the chain above is offered `PanicRecovered` where a refusal gives it
/// `GuardRejection`.
pub(crate) async fn run_guards<T: Transport>(
    entries: &[GuardEntry<T>],
    ctx: &T::Context,
) -> Result<(), GuardFailure> {
    for (index, entry) in entries.iter().enumerate() {
        let guard = match entry {
            GuardEntry::Ready(guard) => guard.clone(),
            GuardEntry::Factory(factory) => factory.create(ctx).await,
        };
        match crate::panic_recovery::catch_async(PipelineSegment::Guard, guard.can_activate(ctx))
            .await
        {
            Ok(true) => {}
            Ok(false) => return Err(GuardFailure::Rejected(GuardRejection::new(index))),
            Err(event) => return Err(GuardFailure::Panicked { index, event }),
        }
    }
    Ok(())
}

/// The interceptors this call runs, outermost first.
pub(crate) async fn interceptors_for<T: Transport>(
    entries: &[InterceptorEntry<T>],
    ctx: &T::Context,
) -> Vec<Arc<dyn Interceptor<T::Context, Answer<T>>>> {
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        out.push(match entry {
            InterceptorEntry::Ready(interceptor) => interceptor.clone(),
            InterceptorEntry::Factory(factory) => factory.create(ctx).await,
        });
    }
    out
}

/// The innermost step of the interceptor chain: resolve the handler and run it.
///
/// One per transport, and each is its own two lines of how to reach a handler — a route's
/// `execute`, a controller resolved per call and asked to handle a message, a gateway asked to
/// handle an event. Everything wrapped around it is [`through_interceptors`], once.
#[async_trait::async_trait]
pub(crate) trait Leaf<T: Transport>: Send + Sync {
    async fn call(&self, ctx: &T::Context) -> Answer<T>;
}

/// Run an error renderer under panic recovery, answering with `fallback` if it panics.
///
/// The renderer is the last thing between the framework and the wire, so a panic in it has nothing
/// left to be remapped by. It is logged, and the transport's fallback answers instead: a literal
/// built from static values that calls no user code.
pub(crate) fn safe_render<R>(render: impl FnOnce() -> R, fallback: fn() -> R) -> R {
    match crate::panic_recovery::catch_sync(PipelineSegment::ResponseRendering, render) {
        Ok(rendered) => rendered,
        Err(panic_event) => {
            tracing::error!(panic = %panic_event.message, "error renderer panicked; answering with the transport's fallback");
            fallback()
        }
    }
}

/// A leaf made of a closure, called at most once: gRPC's handler, packaged by the macro as a
/// delegate the chain hands the call to. `ClosureLeaf` stays private; this is the one way to build
/// it.
pub(crate) fn closure_leaf<T, D, Fut>(delegate: D) -> Arc<dyn Leaf<T>>
where
    T: Transport,
    D: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Answer<T>> + Send + 'static,
{
    Arc::new(ClosureLeaf(parking_lot::Mutex::new(Some(delegate))))
}

struct ClosureLeaf<D>(parking_lot::Mutex<Option<D>>);

#[async_trait::async_trait]
impl<T, D, Fut> Leaf<T> for ClosureLeaf<D>
where
    T: Transport,
    D: FnOnce() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Answer<T>> + Send + 'static,
{
    async fn call(&self, _ctx: &T::Context) -> Answer<T> {
        // `InterceptorNext::run` consumes its box, so the leaf is called at most once and the
        // delegate is present; a second call is a framework bug and answers as a recovered panic
        // would.
        let delegate = self.0.lock().take();
        match delegate {
            Some(delegate) => delegate().await,
            None => Err(T::Error::from(PanicRecovered::with_message(
                PipelineSegment::HandlerBody,
                "the handler was called a second time",
            ))),
        }
    }
}

/// Run `leaf` with `interceptors` wrapped around it, outermost first.
///
/// Each interceptor is handed the rest of the chain and may decline to call it, which is how a
/// cache hit answers without the handler running. A panic in one is recovered here rather than
/// below, so the chain above is offered the event with the segment it came from.
pub(crate) async fn through_interceptors<T: Transport>(
    ctx: &T::Context,
    interceptors: &[Arc<dyn Interceptor<T::Context, Answer<T>>>],
    leaf: Arc<dyn Leaf<T>>,
) -> Answer<T> {
    let Some((first, rest)) = interceptors.split_first() else {
        return leaf.call(ctx).await;
    };

    let next = ChainNext::<T> {
        interceptors: rest.to_vec(),
        leaf: leaf.clone(),
    };

    match crate::panic_recovery::catch_async(
        PipelineSegment::Interceptor,
        first.intercept(ctx, Box::new(next)),
    )
    .await
    {
        Ok(answer) => answer,
        Err(event) => Err(T::Error::from(event)),
    }
}

/// What an interceptor is handed: the interceptors below it, and the leaf under those.
struct ChainNext<T: Transport> {
    interceptors: Vec<Arc<dyn Interceptor<T::Context, Answer<T>>>>,
    leaf: Arc<dyn Leaf<T>>,
}

#[async_trait::async_trait]
impl<T: Transport> crate::enhancer::InterceptorNext<T::Context, Answer<T>> for ChainNext<T> {
    async fn run(self: Box<Self>, ctx: &T::Context) -> Answer<T> {
        through_interceptors::<T>(ctx, &self.interceptors, self.leaf).await
    }
}

/// Walk the chain and answer with the first claim, or `None` if nobody claims.
///
/// Reverse registration order, so the most specific handler is consulted first: a handler declared
/// on the method before one declared on the target, and both before a global. Every transport
/// stacks its tiers into one vector in that order, so reversing it here is the whole rule.
pub(crate) async fn claim<T: Transport>(
    handlers: &[ErrorHandlerArc<T>],
    error: &(dyn std::error::Error + Send + Sync + 'static),
    ctx: &T::Context,
) -> Option<Answer<T>> {
    for (position, handler) in handlers.iter().rev().enumerate() {
        if let Some(claimed) = offer_to::<T>(handler, error, ctx, position).await {
            return Some(claimed);
        }
    }
    None
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
) -> Option<Answer<T>> {
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
