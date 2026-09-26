use crate::dispatch::transport::{
    EnhancerSet, ErrorHandlerArc, GuardEntry, Http, InterceptorEntry,
};
use std::sync::Arc;

use crate::{
    async_trait,
    context::ExecutionContext,
    context::Metadata,
    dispatch::ExecutionResult,
    enhancer::pipeline::{Leaf, through_interceptors},
    enhancer::{Interceptor, pipeline::GuardFailure},
    errors::{Error, MiddlewareFailure, PanicRecovered, PipelineSegment},
    http::Route,
    http::middleware::{Middleware, MiddlewareChain},
    http::{HttpContext, HttpError, HttpMethod, HttpRequest, HttpResponse},
};

/// The innermost step of the chain: the matched route's own handler.
struct RouteLeaf(Arc<dyn Route>);

#[async_trait]
impl Leaf<Http> for RouteLeaf {
    async fn call(&self, context: &HttpContext) -> crate::http::HttpHandlerResult {
        tracing::trace!("executing controller handler");
        match crate::panic_recovery::catch_async(
            PipelineSegment::HandlerBody,
            self.0.execute(context),
        )
        .await
        {
            Ok(ExecutionResult::Ok(response)) => Ok(response),
            Ok(ExecutionResult::Err(http_err)) => Err(http_err),
            Err(event) => Err(HttpError::from(event)),
        }
    }
}

pub(crate) struct RoutePipeline {
    instance: Arc<dyn Route>,
    guards: Vec<GuardEntry<Http>>,
    interceptors: Vec<InterceptorEntry<Http>>,
    middleware_chain: MiddlewareChain,
    error_handlers: Vec<ErrorHandlerArc<Http>>,
    metadata: Arc<Metadata>,
}

impl RoutePipeline {
    /// What the wrapped route declares about its answer — see [`Route::streams`].
    pub(crate) fn streams(&self) -> bool {
        self.instance.streams()
    }

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
        guards: Vec<GuardEntry<Http>>,
        interceptors: Vec<InterceptorEntry<Http>>,
        error_handlers: Vec<ErrorHandlerArc<Http>>,
        metadata: Arc<Metadata>,
    ) -> HttpResponse {
        // The context comes first now: it owns the execution's cache, so a
        // execution-scoped provider injected into a guard and into the controller
        // is constructed once only if both resolve against the same one.
        let context = HttpContext::new(req, metadata.clone());

        // Guards first, one at a time, and nothing below them built until every one has passed:
        // a `Factory` entry is an execution-scoped provider's own resolution, dependencies
        // included, which is the work a refusal exists to avoid.
        let answer = match crate::enhancer::pipeline::run_guards::<Http>(&guards, &context).await {
            Ok(()) => {
                let interceptors =
                    crate::enhancer::pipeline::interceptors_for::<Http>(&interceptors, &context)
                        .await;
                Self::run_chain(&context, instance, interceptors).await
            }
            Err(GuardFailure::Rejected(rejection)) => {
                tracing::debug!(
                    guard_index = rejection.guard_index,
                    "guard rejected request"
                );
                Err(HttpError::from(rejection))
            }
            Err(GuardFailure::Panicked { index, event }) => {
                tracing::debug!(guard_index = index, panic = %event.message, "guard panicked");
                Err(HttpError::from(event))
            }
        };

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
                // Dropped without the body having answered `None`, the exchange ended early.
                // Work feeding the body escaped the handler's future and would otherwise learn
                // only at its next send.
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

    /// The interceptor chain around the handler. Every way this can fail leaves as `Err`, and the
    /// one place that consults the error chain is the caller.
    async fn run_chain(
        context: &HttpContext,
        instance: Arc<dyn Route>,
        interceptors: Vec<Arc<dyn Interceptor<HttpContext, crate::http::HttpHandlerResult>>>,
    ) -> crate::http::HttpHandlerResult {
        if !interceptors.is_empty() {
            tracing::trace!(count = interceptors.len(), "entering interceptor chain");
        }
        through_interceptors::<Http>(context, &interceptors, Arc::new(RouteLeaf(instance))).await
    }

    /// Run the chain on a typed framework event. If nothing claims it, the
    /// event renders itself through the active transport rendering. Reshape a
    /// rejection with `#[catch(GuardRejection)]`.
    async fn handle_framework_event<E>(
        event: E,
        error_handlers: &[ErrorHandlerArc<Http>],
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

    /// Drive a renderer with the shared recovery, falling back to a literal 500.
    fn safe_render<F>(render: F) -> HttpResponse
    where
        F: FnOnce() -> HttpResponse,
    {
        crate::enhancer::pipeline::safe_render(render, Self::fallback_500_response)
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
}
