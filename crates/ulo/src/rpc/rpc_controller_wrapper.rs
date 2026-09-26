use crate::dispatch::Items;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;

use super::{RpcCallInfo, RpcControllerSource, RpcData, RpcError, RpcHandlerResult};
use crate::context::Metadata;
use crate::dispatch::ExecutionResult;
use crate::dispatch::transport::Rpc;
use crate::enhancer::Interceptor;
use crate::enhancer::pipeline::{GuardFailure, Leaf, through_interceptors};
use crate::rpc::RpcContext;
use crate::spi::{RpcErrorHandlerArc, RpcGuardEntry, RpcInterceptorEntry};
use futures::StreamExt;

/// The innermost step of the chain: the controller, resolved for this call, asked to handle it.
struct ControllerLeaf(Arc<dyn RpcControllerSource>);

#[async_trait]
impl Leaf<Rpc> for ControllerLeaf {
    async fn call(&self, context: &RpcContext) -> RpcHandlerResult {
        let controller = self.0.resolve(context).await;
        match crate::panic_recovery::catch_async(
            crate::errors::PipelineSegment::HandlerBody,
            controller.handle_message(context),
        )
        .await
        {
            Ok(ExecutionResult::Ok(output)) => Ok(output),
            Ok(ExecutionResult::Err(rpc_err)) => Err(rpc_err),
            Err(event) => Err(RpcError::from(event)),
        }
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
        // Guards first, one at a time, and no interceptor built until every one has passed.
        let answer = match crate::enhancer::pipeline::run_guards::<Rpc>(&all_guards, &ctx).await {
            Ok(()) => {
                let interceptors =
                    crate::enhancer::pipeline::interceptors_for::<Rpc>(&all_interceptors, &ctx)
                        .await;
                Self::run_chain(&ctx, &self.source, &interceptors).await
            }
            Err(GuardFailure::Rejected(rejection)) => {
                tracing::debug!(
                    guard_index = rejection.guard_index,
                    "guard rejected message"
                );
                Err(RpcError::from(rejection))
            }
            Err(GuardFailure::Panicked { index, event }) => {
                tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                Err(RpcError::from(event))
            }
        };

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
                    Some(Err(reshaped)) => Ok(Items::One(Self::safe_render(|| reshaped.to_data()))),
                    None => Ok(Items::One(Self::safe_render(|| rpc_err.to_data()))),
                }
            }
        };

        // The execution ends when the answer does. A stream has emitted nothing
        // at this point, so the context rides it rather than dying here.
        match answer {
            Ok(Items::Many(stream)) => Ok(Items::Many(
                crate::dispatch::ScopedStream::new(stream, ctx).boxed(),
            )),
            other => other,
        }
    }

    /// The interceptor chain around the handler. Every way this can fail leaves as `Err`.
    async fn run_chain(
        ctx: &RpcContext,
        source: &Arc<dyn RpcControllerSource>,
        interceptors: &[Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>],
    ) -> RpcHandlerResult {
        through_interceptors::<Rpc>(ctx, interceptors, Arc::new(ControllerLeaf(source.clone())))
            .await
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
}
