use crate::dispatch::transport::Rpc;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::{
    RpcCallInfo, RpcControllerSource, RpcData, RpcError, RpcHandlerOutput, RpcHandlerResult,
};
use crate::context::Metadata;
use crate::dispatch::ExecutionResult;
use crate::enhancer::{Guard, Interceptor, InterceptorNext};
use crate::rpc::RpcContext;
use crate::spi::{RpcErrorHandlerArc, RpcGuardEntry, RpcInterceptorEntry};
use futures::StreamExt;
use futures::stream::BoxStream;

/// Delegates to the handler's reply stream while owning the execution's
/// context — cache, extensions, and token stay alive until the last item.
///
/// `BoxStream` is `Pin<Box<_>>` and therefore `Unpin`, so the projection
/// needs no pin machinery.
struct ScopedRpcStream {
    inner: BoxStream<'static, Result<RpcData, RpcError>>,
    context: RpcContext,
    /// Set once the inner stream answers `None`. An error item does not set
    /// it: the adapter stops the drain there and drops this un-drained, so
    /// the producer behind an abnormal end hears the token too.
    drained: bool,
}

impl futures::Stream for ScopedRpcStream {
    type Item = Result<RpcData, RpcError>;

    fn poll_next(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        let this = self.get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_next(cx);
        if matches!(polled, std::task::Poll::Ready(None)) {
            this.drained = true;
        }
        polled
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// A stream dropped with items still to come is the caller having gone —
/// disconnect, cancel notice, or shutdown. Nothing else observes that: the
/// handler returned when it had a stream, and whatever feeds it is not inside
/// the future the adapter drops.
impl Drop for ScopedRpcStream {
    fn drop(&mut self) {
        if !self.drained {
            use crate::context::ExecutionContext as _;
            self.context.cancellation().cancel();
        }
    }
}

struct RpcChainNext {
    interceptors: Vec<Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>>,
    source: Arc<dyn RpcControllerSource>,
}

#[async_trait]
impl InterceptorNext<RpcContext, RpcHandlerResult> for RpcChainNext {
    async fn run(self: Box<Self>, context: &RpcContext) -> RpcHandlerResult {
        RpcControllerWrapper::execute_with_interceptors(context, &self.interceptors, &self.source)
            .await
    }
}

/// Wraps an [`RpcControllerSource`] with the full guard/interceptor pipeline.
pub(crate) struct RpcControllerWrapper {
    source: Arc<dyn RpcControllerSource>,
    guards: Vec<RpcGuardEntry>,
    interceptors: Vec<RpcInterceptorEntry>,
    error_handlers: Vec<RpcErrorHandlerArc>,
    metadata: Arc<Metadata>,
    /// Per-pattern metadata, already merged over `metadata` at expansion. A pattern absent
    /// here declared nothing of its own and reads the controller's.
    handler_metadata: HashMap<String, Arc<Metadata>>,
    handler_guards: HashMap<String, Vec<RpcGuardEntry>>,
    handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>>,
    handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>>,
}

impl RpcControllerWrapper {
    pub(crate) fn new(
        source: Arc<dyn RpcControllerSource>,
        guards: Vec<RpcGuardEntry>,
        interceptors: Vec<RpcInterceptorEntry>,
        error_handlers: Vec<RpcErrorHandlerArc>,
        metadata: Arc<Metadata>,
        handler_metadata: HashMap<String, Arc<Metadata>>,
        handler_guards: HashMap<String, Vec<RpcGuardEntry>>,
        handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>>,
        handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>>,
    ) -> Self {
        Self {
            source,
            guards,
            interceptors,
            error_handlers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
        }
    }

    pub(crate) fn patterns(&self) -> Vec<String> {
        self.source.patterns()
    }

    pub(crate) async fn handle_message(
        &self,
        data: RpcData,
        info: RpcCallInfo,
    ) -> RpcHandlerResult {
        let RpcCallInfo {
            pattern,
            headers,
            extensions,
        } = info;
        let ctx = RpcContext::with_extensions(
            pattern.clone(),
            data,
            headers,
            Some(
                self.handler_metadata
                    .get(&pattern)
                    .unwrap_or(&self.metadata)
                    .clone(),
            ),
            extensions,
        );

        let mut all_guards = self.guards.clone();
        if let Some(h) = self.handler_guards.get(&pattern) {
            all_guards.extend_from_slice(h);
        }
        let mut all_interceptors = self.interceptors.clone();
        if let Some(h) = self.handler_interceptors.get(&pattern) {
            all_interceptors.extend_from_slice(h);
        }
        let mut all_error_handlers = self.error_handlers.clone();
        if let Some(h) = self.handler_error_handlers.get(&pattern) {
            all_error_handlers.extend_from_slice(h);
        }
        let guards = crate::enhancer::pipeline::guards_for::<Rpc>(&all_guards, &ctx).await;
        let interceptors =
            crate::enhancer::pipeline::interceptors_for::<Rpc>(&all_interceptors, &ctx).await;

        let answer = Self::run_chain(&ctx, &self.source, &guards, &interceptors).await;

        // The one place the chain runs. A guard's refusal, a panic from any segment and the
        // handler's own error all arrive as `Err`, so a `#[catch]` handler is offered every one
        // of them and an unclaimed one renders the same way whichever produced it.
        let answer = match answer {
            Ok(output) => Ok(output),
            Err(rpc_err) => {
                let observed: &(dyn std::error::Error + Send + Sync + 'static) = match &rpc_err {
                    RpcError::AppError(e) => e.as_ref(),
                    other => other,
                };
                match crate::enhancer::pipeline::claim::<Rpc>(&all_error_handlers, observed, &ctx)
                    .await
                {
                    // Everything this controller produces is an answer, so it renders as one. A
                    // chain handler that recovered the call answers with its reply; one that
                    // reshaped the failure, and an unclaimed failure, both answer with the
                    // canonical envelope. A wire-`err` frame is reserved for a call that reached
                    // no controller at all.
                    Some(Ok(output)) => Ok(output),
                    Some(Err(reshaped)) => Ok(RpcHandlerOutput::Single(Self::safe_render(|| {
                        reshaped.to_data()
                    }))),
                    None => Ok(RpcHandlerOutput::Single(Self::safe_render(|| {
                        rpc_err.to_data()
                    }))),
                }
            }
        };

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(RpcHandlerOutput::Stream(stream)) => Ok(RpcHandlerOutput::Stream(
                ScopedRpcStream {
                    inner: stream,
                    context: ctx,
                    drained: false,
                }
                .boxed(),
            )),
            other => other,
        }
    }

    /// Guards, then the interceptor chain. Every way this can fail leaves as `Err`.
    async fn run_chain(
        ctx: &RpcContext,
        source: &Arc<dyn RpcControllerSource>,
        guards: &[Arc<dyn Guard<RpcContext>>],
        interceptors: &[Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>],
    ) -> RpcHandlerResult {
        for (index, guard) in guards.iter().enumerate() {
            // A panicking guard is a bug, not a verdict: it takes the same route as any other
            // pipeline panic, so `#[catch(PanicRecovered)]` sees it and a refusal gives the chain
            // `GuardRejection` instead.
            match crate::panic_recovery::catch_async(
                crate::errors::PipelineSegment::Guard,
                guard.can_activate(ctx),
            )
            .await
            {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!(guard_index = index, "guard rejected message");
                    return Err(RpcError::from(crate::errors::GuardRejection::new(index)));
                }
                Err(event) => {
                    tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                    return Err(RpcError::from(event));
                }
            }
        }

        Self::execute_with_interceptors(ctx, interceptors, source).await
    }

    /// Drive `RpcError::to_data` with panic recovery — a panic in the
    /// renderer is the last thing the framework can do for the caller, so it
    /// is logged and a hardcoded `Internal` envelope goes out instead.
    fn safe_render<F>(render: F) -> RpcData
    where
        F: FnOnce() -> RpcData,
    {
        match crate::panic_recovery::catch_sync(
            crate::errors::PipelineSegment::ResponseRendering,
            render,
        ) {
            Ok(data) => data,
            Err(panic_event) => {
                tracing::error!(panic = %panic_event.message, "error renderer panicked; falling back to a bare Internal envelope");
                Self::fallback_internal_data()
            }
        }
    }

    /// Hardcoded fallback envelope when the regular renderer panics.
    ///
    /// Built from static values, so none of the user code the renderer was
    /// calling runs again. It keeps the canonical envelope: an RPC frame
    /// carries no content type, so a bare string would reach the caller's
    /// decoder as a JSON string where every other reply is an object.
    fn fallback_internal_data() -> RpcData {
        RpcData::json(serde_json::json!({
            "status": "error",
            "kind": "Internal",
            "message": "Internal Server Error",
        }))
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` is logged and answers `None`, so the caller continues
    /// to the next handler. Without this, a single bad chain handler would
    /// kill the whole error-recovery path and the original error would
    /// never reach the fallback `to_data` rendering.
    ///
    /// `position` counts from the most specific handler — the chain runs
    /// pattern, then controller, then global — and is logged so a panic names
    /// which registration it came from.
    async fn execute_with_interceptors(
        context: &RpcContext,
        interceptors: &[Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>],
        source: &Arc<dyn RpcControllerSource>,
    ) -> RpcHandlerResult {
        if interceptors.is_empty() {
            return Self::execute_handler(context, source).await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = RpcChainNext {
            interceptors: rest.to_vec(),
            source: source.clone(),
        };

        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(answer) => answer,
            Err(event) => Err(RpcError::from(event)),
        }
    }

    /// Run the user handler. A panic below is a `PanicRecovered` on the `Err` side.
    async fn execute_handler(
        context: &RpcContext,
        source: &Arc<dyn RpcControllerSource>,
    ) -> RpcHandlerResult {
        let controller = source.resolve(context).await;
        let exec_result = crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::HandlerBody,
            controller.handle_message(context),
        )
        .await;
        match exec_result {
            Ok(ExecutionResult::Ok(output)) => Ok(output),
            Ok(ExecutionResult::Err(rpc_err)) => Err(rpc_err),
            Err(event) => Err(RpcError::from(event)),
        }
    }
}
