//! One dispatchable HTTP route, and the enhancers it declares.
//!
//! A `#[controller]` yields one [`Route`] per handler method. Everything the dispatcher reads
//! before a request arrives — the path, the method, the enhancers, the metadata — is on the route,
//! and `execute` resolves the controller instance inside the call being served.

use std::sync::Arc;

use async_trait::async_trait;

use crate::context::Metadata;
use crate::dispatch::{ExecutionResult, Http};
use crate::enhancer::{ErrorHandlerDeclaration, GuardDeclaration, InterceptorDeclaration};
use crate::http::{HttpContext, HttpError, HttpMethod, HttpResponse};

/// What one route declares. A `#[controller]` yields one [`Route`] per handler method, so this is
/// the whole manifest for that method — what the other transports split across a target-level
/// descriptor and a per-handler one.
///
/// Each role is one vector in the order written, and the resolver keeps that order. A guard or
/// interceptor entry is a token, a value or a constructor (see
/// [`GuardDeclaration`](crate::enhancer::GuardDeclaration)); an error-handler entry is a token or a
/// value.
#[derive(Default)]
pub struct RouteEnhancers {
    pub guards: Vec<GuardDeclaration<Http>>,
    pub interceptors: Vec<InterceptorDeclaration<Http>>,
    pub error_handlers: Vec<ErrorHandlerDeclaration<Http>>,
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

    fn enhancers(&self) -> RouteEnhancers {
        RouteEnhancers::default()
    }

    /// Whether this route answers with a body the client reads as it is produced.
    ///
    /// `#[sse]` sets it, and an adapter that collects a body before sending refuses such a route
    /// at registration rather than accepting one it can never answer.
    ///
    /// Read once, at registration, so it covers what a route *declares*. A `#[get]` handler that
    /// returns `Body::stream` decides that per call and is not covered.
    fn streams(&self) -> bool {
        false
    }

    /// What this route declares — roles, permissions, anything a guard or interceptor reads
    /// off the context before the handler runs.
    fn metadata(&self) -> Arc<Metadata> {
        Arc::new(Metadata::new())
    }
}
