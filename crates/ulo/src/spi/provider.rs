use std::{any::Any, future::Future, pin::Pin, sync::Arc};

use async_trait::async_trait;
use rustc_hash::FxHashMap;

use crate::di::Execution;
use crate::enhancer::{ErrorHandler, Guard, Interceptor};
use crate::http::middleware::Middleware;
use crate::{
    di::ProviderScope, grpc::GrpcContext, http::HttpContext, http::HttpResponse, rpc::RpcContext,
    rpc::RpcData, ws::WsContext, ws::WsMessage,
};

#[allow(unused_imports)]
use std::marker::PhantomData;

#[async_trait]
pub trait Provider: Send + Sync {
    fn token(&self) -> String;

    /// The value this provider supplies to the execution `ctx` opens.
    ///
    /// A singleton answers with the value built at startup. An execution-scoped provider builds
    /// one per execution and caches it on `ctx`, so everything in the same call that asks for
    /// this token shares it; a transient one builds on every call. The answer is erased —
    /// callers downcast to the concrete type the token stands for.
    async fn resolve(&self, ctx: Execution) -> Box<dyn Any + Send>;
    fn scope(&self) -> ProviderScope {
        ProviderScope::Singleton
    }

    fn multi_base_token(&self) -> Option<String> {
        None
    }
    fn as_multi_item(&self) -> Option<Arc<dyn Any + Send + Sync>> {
        None
    }

    // Lifecycle hooks — overridden by the macro when the user annotates a method.
    // Default implementations are no-ops so providers without hooks incur no overhead.
    async fn on_module_init(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn on_application_bootstrap(&self) -> crate::di::InitResult {
        Ok(())
    }
    async fn on_module_destroy(&self) {}
    async fn before_application_shutdown(&self, _signal: Option<String>) {}
    async fn on_application_shutdown(&self, _signal: Option<String>) {}
}

// ---- Per-transport entry / factory types (typed registries) ----------------

macro_rules! transport_factory_types {
    (
        $context:ty, $answer:ty,
        $guard_factory:ident, $guard_entry:ident,
        $interceptor_factory:ident, $interceptor_entry:ident
    ) => {
        pub trait $guard_factory: Send + Sync {
            fn create<'a>(
                &'a self,
                ctx: &'a $context,
            ) -> Pin<Box<dyn Future<Output = Arc<dyn Guard<$context> + Send + Sync>> + Send + 'a>>;
        }

        #[derive(Clone)]
        pub enum $guard_entry {
            Ready(Arc<dyn Guard<$context>>),
            Factory(Arc<dyn $guard_factory>),
        }

        pub trait $interceptor_factory: Send + Sync {
            fn create<'a>(
                &'a self,
                ctx: &'a $context,
            ) -> Pin<
                Box<
                    dyn Future<Output = Arc<dyn Interceptor<$context, $answer> + Send + Sync>>
                        + Send
                        + 'a,
                >,
            >;
        }

        #[derive(Clone)]
        pub enum $interceptor_entry {
            Ready(Arc<dyn Interceptor<$context, $answer>>),
            Factory(Arc<dyn $interceptor_factory>),
        }
    };
}

transport_factory_types!(
    HttpContext,
    HttpResponse,
    DynHttpGuardFactory,
    HttpGuardEntry,
    DynHttpInterceptorFactory,
    HttpInterceptorEntry
);

transport_factory_types!(
    RpcContext,
    crate::rpc::RpcHandlerResult,
    DynRpcGuardFactory,
    RpcGuardEntry,
    DynRpcInterceptorFactory,
    RpcInterceptorEntry
);

transport_factory_types!(
    WsContext,
    crate::ws::WsHandlerResult,
    DynWsGuardFactory,
    WsGuardEntry,
    DynWsInterceptorFactory,
    WsInterceptorEntry
);

transport_factory_types!(
    GrpcContext,
    crate::grpc::GrpcHandlerResult,
    DynGrpcGuardFactory,
    GrpcGuardEntry,
    DynGrpcInterceptorFactory,
    GrpcInterceptorEntry
);

pub(crate) type HttpErrorHandlerArc = Arc<dyn ErrorHandler<HttpContext, HttpResponse>>;
pub(crate) type RpcErrorHandlerArc = Arc<dyn ErrorHandler<RpcContext, RpcData>>;
pub(crate) type WsErrorHandlerArc = Arc<dyn ErrorHandler<WsContext, WsMessage>>;
pub(crate) type GrpcErrorHandlerArc = Arc<dyn ErrorHandler<GrpcContext, crate::grpc::GrpcStatus>>;

/// Role trait-objects a provider may contribute to the registry.
///
/// Returned as the second element of `ProviderFactory::build`. The container
/// inserts each variant into the matching slot of `RoleRegistry` keyed by the
/// provider token (or, for gateways, by WS path).
#[derive(Clone)]
pub enum ProviderRole {
    HttpGuard(HttpGuardEntry),
    HttpInterceptor(HttpInterceptorEntry),
    HttpErrorHandler(HttpErrorHandlerArc),

    RpcGuard(RpcGuardEntry),
    RpcInterceptor(RpcInterceptorEntry),
    RpcErrorHandler(RpcErrorHandlerArc),

    WsGuard(WsGuardEntry),
    WsInterceptor(WsInterceptorEntry),
    WsErrorHandler(WsErrorHandlerArc),

    GrpcGuard(GrpcGuardEntry),
    GrpcInterceptor(GrpcInterceptorEntry),
    GrpcErrorHandler(GrpcErrorHandlerArc),

    Middleware(Arc<dyn Middleware>),
    Gateway(Arc<Box<dyn crate::ws::Gateway>>),
}

/// A fully-built, ready-to-inject provider with its role registrations.
///
/// Returned from `ProviderFactory::build` and passed as dep values so
/// wrapper factories (e.g. `provider_alias!`) can forward roles without
/// a downcast.
#[derive(Clone)]
pub struct Injectable {
    pub instance: Arc<Box<dyn Provider>>,
    pub roles: Vec<ProviderRole>,
}

impl Injectable {
    pub fn new(instance: Arc<Box<dyn Provider>>, roles: Vec<ProviderRole>) -> Self {
        Self { instance, roles }
    }
}

#[async_trait]
pub trait ProviderFactory {
    fn token(&self) -> String;
    fn dependency_tokens(&self) -> Vec<String> {
        vec![]
    }
    fn multi_base_token(&self) -> Option<String> {
        None
    }

    /// A fingerprint of this factory's runtime configuration, folded into the identity of the
    /// `DynamicModule` that carries it.
    ///
    /// Two dynamic modules built from the same maker (e.g. `SeaOrmModule::for_root`) share a base
    /// name but must be distinguished by what they were configured with — a database URL, a pool
    /// size. Return a value derived from that config so identical registrations dedup (the same
    /// module reached through two import paths) while different ones stay distinct. `None` (the
    /// default) leaves identity keyed on the base name alone: two such modules with differing
    /// config collapse silently, as before. Integrations that support multiple instances should
    /// override this.
    fn identity_hint(&self) -> Option<String> {
        None
    }

    async fn build(&self, deps: FxHashMap<String, Injectable>) -> Injectable;
}
