//! One dispatchable HTTP route, and the enhancers it declares.
//!
//! A `#[controller]` yields one [`Route`] per handler method. Everything the dispatcher reads
//! before a request arrives — the path, the method, the enhancers, the metadata — is on the route,
//! and `execute` resolves the controller instance inside the call being served.

use std::sync::Arc;

use async_trait::async_trait;

use crate::context::Metadata;
use crate::enhancer::{Guard, Interceptor};
use crate::http::{HttpContext, HttpError, HttpMethod, HttpResponse};
use crate::spi::{ExecutionResult, HttpErrorHandlerArc};

/// Per-route enhancer manifest — both DI-resolved tokens and direct-instantiation arcs.
///
/// `*_tokens` come from `#[use_guards(MyGuard)]`-style attributes that resolve via
/// the DI container; `guards` / `interceptors` / `error_handlers` come
/// from `#[use_guards(MyGuard{})]`-style attributes that bypass DI and instantiate
/// the enhancer inline.
#[derive(Default)]
pub struct ControllerEnhancers {
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub guards: Vec<Arc<dyn Guard<HttpContext>>>,
    pub interceptors: Vec<Arc<dyn Interceptor<HttpContext, HttpResponse>>>,
    pub error_handlers: Vec<HttpErrorHandlerArc>,
}

/// One dispatchable route: the handler plus the routing facts and enhancers the
/// dispatcher needs to register and run it. A `Controller` yields one `Route` per
/// handler method.
#[async_trait]
pub trait Route: Send + Sync {
    /// Run the user handler and return either the rendered success response
    /// or the user's typed error preserved for the dispatcher's chain.
    ///
    /// `ctx` is shared: a context is a handle several participants in one
    /// execution hold at once, and the request body — the only part needing
    /// exclusive access — sits behind a lock.
    ///
    /// Returning is the only way to answer. A response is not a field on the
    /// context, so there is no off-phase write to overrule; an enhancer
    /// short-circuits by returning too.
    async fn execute(&self, ctx: &HttpContext) -> ExecutionResult<HttpResponse, HttpError>;
    fn path(&self) -> String;
    fn method(&self) -> HttpMethod;

    fn enhancers(&self) -> ControllerEnhancers {
        ControllerEnhancers::default()
    }

    /// What this route declares — roles, permissions, anything a guard or interceptor reads
    /// off the context before the handler runs.
    fn metadata(&self) -> Arc<Metadata> {
        Arc::new(Metadata::new())
    }
}
