use crate::dispatch::transport::{EnhancerSet, Http};
use std::sync::Arc;

use crate::{
    async_trait,
    context::ExecutionContext,
    context::Metadata,
    dispatch::ExecutionResult,
    enhancer::{Guard, Interceptor, InterceptorNext},
    errors::{Error, GuardRejection, MiddlewareFailure, PanicRecovered, PipelineSegment},
    http::Route,
    http::middleware::{Middleware, MiddlewareChain},
    http::{HttpContext, HttpError, HttpMethod, HttpRequest, HttpResponse},
    spi::{HttpErrorHandlerArc, HttpGuardEntry, HttpInterceptorEntry},
};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

/// The next step in the interceptor chain after factory entries are resolved.
struct ChainNext {
    interceptors: Vec<Arc<dyn Interceptor<HttpContext, crate::http::HttpHandlerResult>>>,
    instance: Arc<dyn Route>,
}

#[async_trait]
impl InterceptorNext<HttpContext, crate::http::HttpHandlerResult> for ChainNext {
    async fn run(self: Box<Self>, context: &HttpContext) -> crate::http::HttpHandlerResult {
        RoutePipeline::execute_with_interceptors(context, &self.interceptors, &self.instance).await
    }
}

pub(crate) struct RoutePipeline {
    instance: Arc<dyn Route>,
    guards: Vec<HttpGuardEntry>,
    interceptors: Vec<HttpInterceptorEntry>,
    middleware_chain: MiddlewareChain,
    error_handlers: Vec<HttpErrorHandlerArc>,
    metadata: Arc<Metadata>,
}

impl RoutePipeline {
    /// `enhancers` is final: the resolver folded this transport's globals in ahead of what the
    /// route declared, which is the order they run.
    pub(crate) fn new(instance: Arc<dyn Route>, enhancers: EnhancerSet<Http>) -> Self {
        let EnhancerSet {
            guards,
            interceptors,
            error_handlers,
        } = enhancers;

        let metadata = instance.metadata();

        Self {
            instance,
            guards,
            interceptors,
            middleware_chain: MiddlewareChain::new(),
            error_handlers,
            metadata,
        }
    }

    pub(crate) fn path(&self) -> String {
        self.instance.path()
    }

    pub(crate) fn method(&self) -> HttpMethod {
        self.instance.method()
    }

    pub(crate) fn set_middleware(&mut self, middleware: Vec<Arc<dyn Middleware>>) {
        for m in middleware {
            self.middleware_chain.use_middleware(m);
        }
    }

    pub(crate) async fn handle_request(&self, req: HttpRequest) -> HttpResponse {
        let method = self.method();
        let path = self.path();
        tracing::debug!(method = %method.as_str(), path = %path, "incoming request");

        let instance = self.instance.clone();
        let guards = self.guards.clone();
        let interceptors = self.interceptors.clone();
        let error_handlers = self.error_handlers.clone();
        let metadata = self.metadata.clone();

        let middleware_result = self
            .middleware_chain
            .execute(req, move |req| {
                Box::pin(async move {
                    Self::execute_controller_logic(
                        req,
                        instance,
                        guards,
                        interceptors,
                        error_handlers,
                        metadata,
                    )
                    .await
                })
            })
            .await;

        match middleware_result {
            Ok(response) => {
                tracing::debug!(method = %method.as_str(), path = %path, status = response.status, "request completed");
                response
            }
            Err(e) => {
                // If the middleware bubbled an `HttpError`, render it directly
                // without going through the chain — the user constructed the
                // wire shape themselves.
                if let Some(http_err) = e.downcast_ref::<HttpError>() {
                    return Self::safe_render(|| http_err.to_response());
                }

                // Middleware failed before the request body could be split; we have no
                // parts to thread through to the execution context. Construct a stub
                // from a minimal request so error handlers still get a typed context.
                let stub = http::Request::builder().body(()).unwrap();
                let error_ctx = HttpContext::from_parts(stub.into_parts().0);

                // A caught middleware panic keeps its own type through the
                // chain, so `#[catch(PanicRecovered)]` matches it the way it
                // matches one from a guard or an interceptor.
                let e = match e.downcast::<PanicRecovered>() {
                    Ok(panic) => {
                        return Self::handle_framework_event(
                            *panic,
                            &self.error_handlers,
                            &error_ctx,
                        )
                        .await;
                    }
                    Err(e) => e,
                };

                let event = MiddlewareFailure::new(e.to_string());
                match crate::enhancer::pipeline::claim::<Http>(
                    &self.error_handlers,
                    &event,
                    &error_ctx,
                )
                .await
                {
                    Some(Ok(response)) => response,
                    Some(Err(reshaped)) => Self::safe_render(|| reshaped.to_response()),
                    None => Self::safe_render(|| crate::http::error::render_error(&event)),
                }
            }
        }
    }

    async fn execute_controller_logic(
        req: HttpRequest,
        instance: Arc<dyn Route>,
        guards: Vec<HttpGuardEntry>,
        interceptors: Vec<HttpInterceptorEntry>,
        error_handlers: Vec<HttpErrorHandlerArc>,
        metadata: Arc<Metadata>,
    ) -> HttpResponse {
        // The context comes first now: it owns the execution's cache, so a
        // execution-scoped provider injected into a guard and into the controller
        // is constructed once only if both resolve against the same one.
        let context = HttpContext::new(req, metadata.clone());

        let guards = crate::enhancer::pipeline::guards_for::<Http>(&guards, &context).await;
        let interceptors =
            crate::enhancer::pipeline::interceptors_for::<Http>(&interceptors, &context).await;

        let answer = Self::run_chain(&context, instance, guards, interceptors).await;

        // The one place the error chain runs. A guard's rejection, an interceptor's refusal, a
        // handler's own error and a panic from any of them all arrive here as `Err`, so a
        // `#[catch]` handler sees every one of them — which it did not when each segment rendered
        // whatever it had produced.
        let response = match answer {
            Ok(response) => response,
            Err(http_err) => {
                // The chain is offered the domain error where one is wrapped, so
                // `#[catch(MyError)]` matches what the handler actually raised.
                let observed: &(dyn std::error::Error + Send + Sync + 'static) = match &http_err {
                    HttpError::AppError(e) => e.as_ref(),
                    other => other,
                };
                match crate::enhancer::pipeline::claim::<Http>(&error_handlers, observed, &context)
                    .await
                {
                    Some(Ok(claimed)) => claimed,
                    Some(Err(reshaped)) => Self::safe_render(|| reshaped.to_response()),
                    None => Self::safe_render(|| http_err.to_response()),
                }
            }
        };

        // The execution ends when the answer does. A streaming body has produced
        // nothing yet at this point, so the context rides it to the last frame
        // rather than dying here with the handler.
        match response.body {
            Some(body) => {
                // Dropped owing frames, the client is gone. Work feeding the body escaped the
                // handler's future and would otherwise learn only at its next send.
                let cancellation = context.cancellation().clone();
                HttpResponse {
                    body: Some(
                        body.keep_alive(context)
                            .on_abandoned(move || cancellation.cancel()),
                    ),
                    ..response
                }
            }
            None => response,
        }
    }

    /// Guards, then the interceptor chain. Every way this can fail leaves as `Err`, and the one
    /// place that consults the error chain is the caller.
    async fn run_chain(
        context: &HttpContext,
        instance: Arc<dyn Route>,
        guards: Vec<Arc<dyn Guard<HttpContext>>>,
        interceptors: Vec<Arc<dyn Interceptor<HttpContext, crate::http::HttpHandlerResult>>>,
    ) -> crate::http::HttpHandlerResult {
        for (i, guard) in guards.iter().enumerate() {
            // `can_activate` is user code — catch panics so the request doesn't tear down. A
            // panicking guard is a hard rejection, and the chain sees `PanicRecovered` where a
            // refusal gives it `GuardRejection`.
            let activated = match crate::panic_recovery::catch_async(
                PipelineSegment::Guard,
                guard.can_activate(&context),
            )
            .await
            {
                Ok(b) => b,
                Err(event) => {
                    tracing::debug!(guard_index = i, panic = %event.message, "guard panicked");
                    return Err(HttpError::from(event));
                }
            };
            if !activated {
                tracing::debug!(guard_index = i, "guard rejected request");
                return Err(HttpError::from(GuardRejection::new(i)));
            }
        }

        if !interceptors.is_empty() {
            tracing::trace!(count = interceptors.len(), "entering interceptor chain");
        }
        Self::execute_with_interceptors(context, &interceptors, &instance).await
    }

    /// Run the chain on a typed framework event. If nothing claims it, the
    /// event renders itself through the active transport rendering. Reshape a
    /// rejection with `#[catch(GuardRejection)]`.
    async fn handle_framework_event<E>(
        event: E,
        error_handlers: &[HttpErrorHandlerArc],
        ctx: &HttpContext,
    ) -> HttpResponse
    where
        E: Error,
    {
        match crate::enhancer::pipeline::claim::<Http>(&error_handlers, &event, ctx).await {
            Some(Ok(handled)) => handled,
            Some(Err(reshaped)) => Self::safe_render(|| reshaped.to_response()),
            None => Self::safe_render(|| crate::http::error::render_error(&event)),
        }
    }

    /// Drive the transport's error renderer with panic recovery. A panic
    /// inside `HttpError::to_response` (or the free-function
    /// `render_error`) would otherwise tear the dispatcher down — the
    /// renderer is the last thing standing between the framework and the
    /// wire, so there's nothing left to remap if it fails. Policy: log the
    /// panic and substitute a minimal hardcoded 500 envelope so the client
    /// still gets a structured reply.
    fn safe_render<F>(render: F) -> HttpResponse
    where
        F: FnOnce() -> HttpResponse,
    {
        match crate::panic_recovery::catch_sync(PipelineSegment::ResponseRendering, render) {
            Ok(resp) => resp,
            Err(panic_event) => {
                tracing::error!(panic = %panic_event.message, "error renderer panicked; falling back to a bare 500");
                Self::fallback_500_response()
            }
        }
    }

    /// Minimal hardcoded 500 used when the regular renderer panics.
    ///
    /// The envelope is a string literal rather than a rendered value: the
    /// renderer that just panicked was calling the error's own `kind()` and
    /// `message()`, and this calls neither, so a recursive panic here is
    /// structurally impossible. It keeps the canonical shape because a client
    /// decoding `{statusCode, message, error}` should not meet a different
    /// body on the one path it cannot anticipate.
    fn fallback_500_response() -> HttpResponse {
        HttpResponse {
            body: Some(
                crate::http::Body::text(
                    r#"{"statusCode":500,"message":"Internal Server Error","error":"Internal Server Error"}"#,
                )
                .with_content_type("application/json"),
            ),
            status: 500,
            headers: vec![],
        }
    }

    /// Run one chain handler with panic recovery: a panicking
    /// `handle_error` is logged and answers `None`, so the caller continues
    /// to the next handler. Without this, a single bad chain handler would
    /// kill the whole error-recovery path and the original error would
    /// never reach the fallback rendering.
    ///
    /// `position` counts from the most specific handler — the chain runs
    /// method, then controller, then global — and is logged so a panic names
    /// which registration it came from.
    /// Onion/Russian doll dispatch through the interceptor chain.
    ///
    /// An interceptor's panic becomes the `Err` side, as a deliberate refusal already is, so both
    /// reach the one chain above rather than one being rendered here and the other not.
    async fn execute_with_interceptors(
        context: &HttpContext,
        interceptors: &[Arc<dyn Interceptor<HttpContext, crate::http::HttpHandlerResult>>],
        instance: &Arc<dyn Route>,
    ) -> crate::http::HttpHandlerResult {
        if interceptors.is_empty() {
            return Self::execute_handler(context, instance).await;
        }

        let (first, rest) = interceptors.split_first().unwrap();

        let next = ChainNext {
            interceptors: rest.to_vec(),
            instance: instance.clone(),
        };

        match crate::panic_recovery::catch_async(
            PipelineSegment::Middleware,
            first.intercept(context, Box::new(next)),
        )
        .await
        {
            Ok(answer) => answer,
            Err(event) => Err(HttpError::from(event)),
        }
    }

    /// Run the user handler. A panic below is a `PanicRecovered` on the `Err` side, and the chain
    /// that might claim it runs once, above the interceptors.
    async fn execute_handler(
        context: &HttpContext,
        instance: &Arc<dyn Route>,
    ) -> crate::http::HttpHandlerResult {
        tracing::trace!("executing controller handler");
        // `AssertUnwindSafe`: handler bodies aren't required to be unwind-safe
        // and adding `RefUnwindSafe` bounds to user code would be punitive.
        // We trust the application to set its own state to a sane shape after
        // a panic — this layer only ensures the panic doesn't escape the
        // dispatcher.
        let exec_result = AssertUnwindSafe(instance.execute(context))
            .catch_unwind()
            .await;
        match exec_result {
            Ok(ExecutionResult::Ok(response)) => Ok(response),
            Ok(ExecutionResult::Err(http_err)) => Err(http_err),
            Err(payload) => Err(HttpError::from(PanicRecovered::from_panic_payload(
                PipelineSegment::HandlerBody,
                payload,
            ))),
        }
    }
}
