//! What a gRPC service declares, and how it is registered with tonic.
//!
//! Each service is declared in its module's `controllers:` list and hands its source over as
//! `Dispatch::Grpc`. The framework resolves the declared enhancer tokens against the role registry
//! at create, stores the `(source, enhancers)` pair, and hands it back at bind through
//! [`register_with`](GrpcServiceSource::register_with) so the macro-generated body can wrap itself
//! in an enhancer-aware tonic service.
//!
//! Nothing here reaches an instance. Registration happens before any call exists, so a service
//! built per call has nothing to answer with yet; the source knows the declarations without one and
//! produces instances later, inside the call being served.
//!
//! The `dyn Any` registrar keeps tonic types out of ulo core. The macro emits a body that
//! downcasts to `tonic::service::RoutesBuilder` and calls
//! `add_service(MyServiceServer::new(wrapper))`, where `wrapper` carries the source and the
//! resolved enhancers. Ulo core never names tonic.

use std::sync::Arc;

use crate::enhancer::{ErrorHandler, Guard, Interceptor};
use crate::grpc::{GrpcContext, GrpcHandlerResult, GrpcStatus};
use crate::spi::{GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry};
/// Per-service bundle of resolved enhancer instances. Built by the framework
/// at create from [`GrpcServiceSource::enhancers`] and handed to
/// [`GrpcServiceSource::register_with`] so the macro-generated wrapper can
/// invoke them per call without touching the DI container at request time.
#[derive(Default, Clone)]
pub struct ResolvedGrpcEnhancers {
    /// Service-level guards; run on every method.
    pub(crate) guards: Vec<GrpcGuardEntry>,
    /// Method-level guards keyed by the handler's Rust method name, which is what
    /// [`GrpcHandlerEnhancers::method`] carries and what the generated wrapper looks up with.
    pub(crate) handler_guards: std::collections::HashMap<String, Vec<GrpcGuardEntry>>,
    /// Service-level interceptors; wrap every method's user delegation.
    pub(crate) interceptors: Vec<GrpcInterceptorEntry>,
    /// Method-level interceptors. Stack on top of service-level (controller-
    /// level entries run first, method-level entries run inside).
    pub(crate) handler_interceptors: std::collections::HashMap<String, Vec<GrpcInterceptorEntry>>,
    /// Service-level error handlers; fire on user-returned `Err` or caught
    /// handler panic. First handler to claim wins (chain runs in reverse
    /// registration order, matching the RPC/HTTP convention).
    pub(crate) error_handlers: Vec<GrpcErrorHandlerArc>,
    /// Method-level error handlers. Composed with service-level into one
    /// reverse-order chain per call.
    pub(crate) handler_error_handlers: std::collections::HashMap<String, Vec<GrpcErrorHandlerArc>>,
}

/// What a gRPC service declares, read once at create. Service-level entries apply to every method;
/// each `handlers` entry adds to one method. A flat descriptor instead of seven accessor methods —
/// the macro builds it, the resolver reads it once.
///
/// Each role arrives two ways. `*_tokens` come from `#[use_guards(MyGuard)]` and resolve against
/// the DI container, so the enhancer may hold injected dependencies. `guards` / `interceptors` /
/// `error_handlers` come from `#[use_guards(MyGuard{})]`, which builds the value at the
/// declaration site and never consults the container. The resolver runs the DI-resolved ones
/// first.
#[derive(Default)]
pub struct GrpcEnhancers {
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub guards: Vec<Arc<dyn Guard<GrpcContext>>>,
    pub interceptors: Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>>,
    pub error_handlers: Vec<Arc<dyn ErrorHandler<GrpcContext, GrpcHandlerResult>>>,
    pub handlers: Vec<GrpcHandlerEnhancers>,
}

/// What one handler declares on top of its service's, keyed by method. Same two ways in as
/// [`GrpcEnhancers`].
#[derive(Default)]
pub struct GrpcHandlerEnhancers {
    /// The handler's Rust method name, which is the key the generated wrapper resolves by.
    pub method: String,
    pub guard_tokens: Vec<String>,
    pub interceptor_tokens: Vec<String>,
    pub error_handler_tokens: Vec<String>,
    pub guards: Vec<Arc<dyn Guard<GrpcContext>>>,
    pub interceptors: Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>>,
    pub error_handlers: Vec<Arc<dyn ErrorHandler<GrpcContext, GrpcHandlerResult>>>,
}

/// A gRPC service's declarations plus its registration hook — implemented by `#[grpc_methods]` on
/// a companion generated beside the service struct, not on the struct itself. The framework
/// discovers sources through the DI container at bind time.
pub trait GrpcServiceSource: Send + Sync + 'static {
    /// Stable token for DI resolution. Defaults to the type name.
    fn token(&self) -> String;

    /// Register this service with the gRPC adapter's routes builder.
    ///
    /// `registrar` is `&mut tonic::service::RoutesBuilder` boxed as `dyn Any`
    /// so ulo core stays tonic-free. The macro-generated impl downcasts and
    /// adds itself, wrapping in an enhancer-aware shim built from
    /// `enhancers`.
    fn register_with(
        &self,
        registrar: &mut dyn std::any::Any,
        enhancers: Arc<ResolvedGrpcEnhancers>,
    );

    /// All enhancer tokens for this service — service-level plus per-method — read once at
    /// startup. Default is empty: a service with no declared enhancers, and the shape a
    /// hand-written source can leave alone.
    fn enhancers(&self) -> GrpcEnhancers {
        GrpcEnhancers::default()
    }
}
