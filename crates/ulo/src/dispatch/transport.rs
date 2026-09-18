//! The four transports as types, and the enhancer plumbing keyed on them.
//!
//! A guard, an interceptor and an error handler differ across HTTP, RPC, WebSocket and gRPC in two
//! ways: the context a handler is given, and the type an interceptor answers with. [`Transport`]
//! names both, so one generic type carries any of them between `create` and dispatch.
//!
//! `HttpGuardEntry` and its seven siblings are aliases of those generic types. A macro expansion
//! names one of them rather than a type and its parameter.

use std::{future::Future, pin::Pin, sync::Arc};

use crate::context::ExecutionContext;
use crate::enhancer::{Guard, Interceptor};
use crate::errors::PanicRecovered;
use crate::{grpc::GrpcContext, http::HttpContext, rpc::RpcContext, ws::WsContext};

/// One transport, as a type.
///
/// Implemented by the four markers below and by nothing else. A transport brings a context, an
/// adapter trait and a wire format with it, which is not something an integration crate adds.
pub trait Transport: 'static {
    /// What a handler, guard, interceptor and error handler on this transport are given.
    type Context: ExecutionContext;

    /// What this transport answers with when the call succeeds.
    type Output: Send;

    /// What it fails with.
    ///
    /// A guard's rejection, an interceptor's refusal and a panic anywhere below all arrive here,
    /// so the error chain runs once above the interceptors rather than at each level that could
    /// produce one. `From<PanicRecovered>` is what lets a walk that catches a panic build the
    /// failure without asking the transport how.
    type Error: Send + From<PanicRecovered>;

    /// How a diagnostic names this transport.
    const NAME: &'static str;
}

/// What a call on `T` answers with.
///
/// Fallible on every transport, which is the shape [`Transport::Error`] exists to fix. Spelled as a
/// free alias rather than an associated type with a default, because associated-type defaults are
/// unstable.
pub type Answer<T> = Result<<T as Transport>::Output, <T as Transport>::Error>;

/// HTTP, served by an `HttpAdapter`.
pub struct Http;
impl Transport for Http {
    type Context = HttpContext;
    type Output = crate::http::HttpResponse;
    type Error = crate::http::HttpError;
    const NAME: &'static str = "HTTP";
}

/// Pattern-addressed RPC, served by an `RpcAdapter`.
pub struct Rpc;
impl Transport for Rpc {
    type Context = RpcContext;
    type Output = crate::rpc::RpcHandlerOutput;
    type Error = crate::rpc::RpcError;
    const NAME: &'static str = "RPC";
}

/// WebSocket, served by a same-port `HttpAdapter` or a separate-port `WsAdapter`.
pub struct Ws;
impl Transport for Ws {
    type Context = WsContext;
    type Output = crate::ws::WsHandlerOutput;
    type Error = crate::ws::WsError;
    const NAME: &'static str = "WS";
}

/// gRPC, served by a `GrpcAdapter`.
pub struct Grpc;
impl Transport for Grpc {
    type Context = GrpcContext;
    type Output = crate::grpc::GrpcReply;
    type Error = crate::grpc::GrpcStatus;
    const NAME: &'static str = "gRPC";
}

/// Builds a guard inside the execution being served.
///
/// The arm a guard reaches when it carries execution-scoped dependencies of its own. A guard
/// without them is stored [`Ready`](GuardEntry::Ready) and shared by every call.
pub trait GuardFactory<T: Transport>: Send + Sync {
    fn create<'a>(
        &'a self,
        ctx: &'a T::Context,
    ) -> Pin<Box<dyn Future<Output = Arc<dyn Guard<T::Context> + Send + Sync>> + Send + 'a>>;
}

/// Builds an interceptor inside the execution being served. See [`GuardFactory`].
pub trait InterceptorFactory<T: Transport>: Send + Sync {
    fn create<'a>(
        &'a self,
        ctx: &'a T::Context,
    ) -> Pin<
        Box<
            dyn Future<Output = Arc<dyn Interceptor<T::Context, Answer<T>> + Send + Sync>>
                + Send
                + 'a,
        >,
    >;
}

/// A guard as the framework stores it: one instance shared by every call, or a factory asked per
/// call.
pub enum GuardEntry<T: Transport> {
    Ready(Arc<dyn Guard<T::Context>>),
    Factory(Arc<dyn GuardFactory<T>>),
}

// `#[derive(Clone)]` bounds every parameter it sees, and `T` is a marker held in no field, so a
// derived impl would demand `Transport: Clone` of callers that never hold one.
impl<T: Transport> Clone for GuardEntry<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Ready(guard) => Self::Ready(guard.clone()),
            Self::Factory(factory) => Self::Factory(factory.clone()),
        }
    }
}

/// An interceptor as the framework stores it. See [`GuardEntry`].
pub enum InterceptorEntry<T: Transport> {
    Ready(Arc<dyn Interceptor<T::Context, Answer<T>>>),
    Factory(Arc<dyn InterceptorFactory<T>>),
}

impl<T: Transport> Clone for InterceptorEntry<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Ready(interceptor) => Self::Ready(interceptor.clone()),
            Self::Factory(factory) => Self::Factory(factory.clone()),
        }
    }
}

/// An error handler as the framework stores it. One instance, shared by every call: unlike a guard
/// or an interceptor, an error handler has no per-call arm.
pub(crate) type ErrorHandlerArc<T> =
    Arc<dyn crate::enhancer::ErrorHandler<<T as Transport>::Context, Answer<T>>>;

/// Every enhancer one transport has registered, keyed by the token a declaration names.
pub(crate) struct EnhancerRegistry<T: Transport> {
    pub(crate) guards: rustc_hash::FxHashMap<String, GuardEntry<T>>,
    pub(crate) interceptors: rustc_hash::FxHashMap<String, InterceptorEntry<T>>,
    pub(crate) error_handlers: rustc_hash::FxHashMap<String, ErrorHandlerArc<T>>,
}

impl<T: Transport> Default for EnhancerRegistry<T> {
    fn default() -> Self {
        Self {
            guards: rustc_hash::FxHashMap::default(),
            interceptors: rustc_hash::FxHashMap::default(),
            error_handlers: rustc_hash::FxHashMap::default(),
        }
    }
}

/// One transport's guards, interceptors and error handlers, in the order they run.
///
/// The shape both tiers take: what a transport runs on every dispatch target, and what one target
/// resolved from its own declarations.
pub(crate) struct EnhancerSet<T: Transport> {
    pub(crate) guards: Vec<GuardEntry<T>>,
    pub(crate) interceptors: Vec<InterceptorEntry<T>>,
    pub(crate) error_handlers: Vec<ErrorHandlerArc<T>>,
}

impl<T: Transport> EnhancerSet<T> {
    /// Append everything in `other`, keeping this set's entries ahead of it.
    pub(crate) fn extend_from(&mut self, other: &Self) {
        self.guards.extend(other.guards.iter().cloned());
        self.interceptors.extend(other.interceptors.iter().cloned());
        self.error_handlers
            .extend(other.error_handlers.iter().cloned());
    }
}

impl<T: Transport> Clone for EnhancerSet<T> {
    fn clone(&self) -> Self {
        Self {
            guards: self.guards.clone(),
            interceptors: self.interceptors.clone(),
            error_handlers: self.error_handlers.clone(),
        }
    }
}

impl<T: Transport> Default for EnhancerSet<T> {
    fn default() -> Self {
        Self {
            guards: Vec::new(),
            interceptors: Vec::new(),
            error_handlers: Vec::new(),
        }
    }
}

pub type HttpGuardEntry = GuardEntry<Http>;
pub type HttpInterceptorEntry = InterceptorEntry<Http>;
pub type RpcGuardEntry = GuardEntry<Rpc>;
pub type RpcInterceptorEntry = InterceptorEntry<Rpc>;
pub type WsGuardEntry = GuardEntry<Ws>;
pub type WsInterceptorEntry = InterceptorEntry<Ws>;
pub type GrpcGuardEntry = GuardEntry<Grpc>;
pub type GrpcInterceptorEntry = InterceptorEntry<Grpc>;
pub(crate) type HttpErrorHandlerArc = ErrorHandlerArc<Http>;
pub(crate) type RpcErrorHandlerArc = ErrorHandlerArc<Rpc>;
pub(crate) type WsErrorHandlerArc = ErrorHandlerArc<Ws>;
pub(crate) type GrpcErrorHandlerArc = ErrorHandlerArc<Grpc>;
