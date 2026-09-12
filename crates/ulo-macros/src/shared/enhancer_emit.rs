//! Single source of truth for the per-transport enhancer emission shape.
//!
//! Three transports × two enhancer roles (Guard / Interceptor) gives six
//! `EnhancerKind` variants, plus gRPC's two. Three transports × ErrorHandler gives three
//! `ErrorHandlerKind` variants. ErrorHandlers don't have entry-wrapping or
//! dyn-factory shapes, so they're a separate small kind to keep `EnhancerKind`
//! uniform.
//!
//! Every emission site (singleton role-push, request-scoped dyn-factory,
//! `provider_factory!` ready, `provider_factory!` non-caching factory) reads
//! from these specs instead of restating the per-variant constants inline.

use proc_macro2::TokenStream;
use quote::quote;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnhancerKind {
    HttpGuard,
    HttpInterceptor,
    RpcGuard,
    RpcInterceptor,
    WsGuard,
    WsInterceptor,
    GrpcGuard,
    GrpcInterceptor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorHandlerKind {
    Http,
    Rpc,
    Ws,
    Grpc,
}

pub struct EnhancerSpec {
    /// `::ulo::traits::ProviderRole::HttpGuard` etc.
    pub role_variant: TokenStream,
    /// `::ulo::__enhancer::HttpGuardEntry` etc.
    pub entry_path: TokenStream,
    /// `::ulo::traits::Guard<::ulo::context::HttpContext>` etc.
    pub trait_path: TokenStream,
    /// `::ulo::__enhancer::DynHttpGuardFactory` etc.
    pub dyn_factory_trait: TokenStream,
    /// Camel-case suffix used to derive a unique factory struct name per kind.
    pub factory_suffix: &'static str,
    /// `::ulo::context::HttpContext` etc. — what this kind's factory is handed.
    pub context_path: TokenStream,
    /// `::ulo::ProviderContext::Http` etc. — how that context is wrapped for
    /// the provider being built.
    pub provider_ctx_variant: TokenStream,
}

pub struct ErrorHandlerSpec {
    /// `::ulo::traits::ProviderRole::HttpErrorHandler` etc.
    pub role_variant: TokenStream,
}

impl EnhancerKind {
    pub fn all() -> [EnhancerKind; 8] {
        use EnhancerKind::*;
        [
            HttpGuard,
            HttpInterceptor,
            RpcGuard,
            RpcInterceptor,
            WsGuard,
            WsInterceptor,
            GrpcGuard,
            GrpcInterceptor,
        ]
    }

    pub fn spec(self) -> EnhancerSpec {
        match self {
            EnhancerKind::HttpGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::HttpGuard },
                entry_path: quote! { ::ulo::__enhancer::HttpGuardEntry },
                trait_path: quote! { ::ulo::traits::Guard<::ulo::context::HttpContext> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynHttpGuardFactory },
                factory_suffix: "HttpGuard",
                context_path: quote! { ::ulo::context::HttpContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::Http },
            },
            EnhancerKind::HttpInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::HttpInterceptor },
                entry_path: quote! { ::ulo::__enhancer::HttpInterceptorEntry },
                trait_path: quote! { ::ulo::traits::Interceptor<::ulo::context::HttpContext, ::ulo::HttpResponse> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynHttpInterceptorFactory },
                factory_suffix: "HttpInterceptor",
                context_path: quote! { ::ulo::context::HttpContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::Http },
            },
            EnhancerKind::RpcGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::RpcGuard },
                entry_path: quote! { ::ulo::__enhancer::RpcGuardEntry },
                trait_path: quote! { ::ulo::traits::Guard<::ulo::context::RpcContext> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynRpcGuardFactory },
                factory_suffix: "RpcGuard",
                context_path: quote! { ::ulo::context::RpcContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::Rpc },
            },
            EnhancerKind::RpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::RpcInterceptor },
                entry_path: quote! { ::ulo::__enhancer::RpcInterceptorEntry },
                trait_path: quote! { ::ulo::traits::Interceptor<::ulo::context::RpcContext, ::ulo::rpc::RpcHandlerResult> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynRpcInterceptorFactory },
                factory_suffix: "RpcInterceptor",
                context_path: quote! { ::ulo::context::RpcContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::Rpc },
            },
            EnhancerKind::WsGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::WsGuard },
                entry_path: quote! { ::ulo::__enhancer::WsGuardEntry },
                trait_path: quote! { ::ulo::traits::Guard<::ulo::ws::WsContext> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynWsGuardFactory },
                factory_suffix: "WsGuard",
                context_path: quote! { ::ulo::ws::WsContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::WebSocket },
            },
            EnhancerKind::WsInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::WsInterceptor },
                entry_path: quote! { ::ulo::__enhancer::WsInterceptorEntry },
                trait_path: quote! { ::ulo::traits::Interceptor<::ulo::ws::WsContext, ::ulo::ws::WsHandlerResult> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynWsInterceptorFactory },
                factory_suffix: "WsInterceptor",
                context_path: quote! { ::ulo::ws::WsContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::WebSocket },
            },
            EnhancerKind::GrpcGuard => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::GrpcGuard },
                entry_path: quote! { ::ulo::__enhancer::GrpcGuardEntry },
                trait_path: quote! { ::ulo::traits::Guard<::ulo::grpc::GrpcContext> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynGrpcGuardFactory },
                factory_suffix: "GrpcGuard",
                context_path: quote! { ::ulo::grpc::GrpcContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::Grpc },
            },
            EnhancerKind::GrpcInterceptor => EnhancerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::GrpcInterceptor },
                entry_path: quote! { ::ulo::__enhancer::GrpcInterceptorEntry },
                trait_path: quote! { ::ulo::traits::Interceptor<::ulo::grpc::GrpcContext, ::ulo::GrpcHandlerResult> },
                dyn_factory_trait: quote! { ::ulo::__enhancer::DynGrpcInterceptorFactory },
                factory_suffix: "GrpcInterceptor",
                context_path: quote! { ::ulo::grpc::GrpcContext },
                provider_ctx_variant: quote! { ::ulo::ProviderContext::Grpc },
            },
        }
    }
}

impl ErrorHandlerKind {
    pub fn all() -> [ErrorHandlerKind; 4] {
        [
            ErrorHandlerKind::Http,
            ErrorHandlerKind::Rpc,
            ErrorHandlerKind::Ws,
            ErrorHandlerKind::Grpc,
        ]
    }

    pub fn spec(self) -> ErrorHandlerSpec {
        match self {
            ErrorHandlerKind::Http => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::HttpErrorHandler },
            },
            ErrorHandlerKind::Rpc => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::RpcErrorHandler },
            },
            ErrorHandlerKind::Ws => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::WsErrorHandler },
            },
            ErrorHandlerKind::Grpc => ErrorHandlerSpec {
                role_variant: quote! { ::ulo::traits::ProviderRole::GrpcErrorHandler },
            },
        }
    }
}

/// Emit the value-probe detection block that pushes a `ProviderRole` for every enhancer trait the
/// already-built `instance` implements. `instance` (an `Arc<ConcreteType>`) and `__roles`
/// (`Vec<ProviderRole>`) must be in scope; the concrete type must be statically known here, so the
/// `ulo::__detect` autoref probes resolve (a generic wrapper would erase the bound and detect
/// nothing — see the `__detect` module docs).
///
/// Shared by every singleton role-registration site: the `#[injectable]` factory and the caching
/// `provider_factory!` factory. Middleware, the eight guard/interceptor
/// kinds, and the four error-handler kinds are each probed; only implemented ones register.
pub fn value_probe_detection() -> TokenStream {
    let mut detects = vec![quote! {
        if let Some(__r) = ::ulo::__detect::MiddlewareProbe(instance.clone()).detect() {
            __roles.push(::ulo::traits::ProviderRole::Middleware(__r));
        }
    }];

    for kind in EnhancerKind::all() {
        let spec = kind.spec();
        let probe = quote::format_ident!("{}Probe", spec.factory_suffix);
        let role_variant = &spec.role_variant;
        let entry_path = &spec.entry_path;
        detects.push(quote! {
            if let Some(__r) = ::ulo::__detect::#probe(instance.clone()).detect() {
                __roles.push(#role_variant(#entry_path::Ready(__r)));
            }
        });
    }
    for kind in ErrorHandlerKind::all() {
        let spec = kind.spec();
        let probe = match kind {
            ErrorHandlerKind::Http => quote::format_ident!("HttpErrorHandlerProbe"),
            ErrorHandlerKind::Rpc => quote::format_ident!("RpcErrorHandlerProbe"),
            ErrorHandlerKind::Ws => quote::format_ident!("WsErrorHandlerProbe"),
            ErrorHandlerKind::Grpc => quote::format_ident!("GrpcErrorHandlerProbe"),
        };
        let role_variant = &spec.role_variant;
        detects.push(quote! {
            if let Some(__r) = ::ulo::__detect::#probe(instance.clone()).detect() {
                __roles.push(#role_variant(__r));
            }
        });
    }

    quote! {
        {
            use ::ulo::__detect::prelude::*;
            #(#detects)*
        }
    }
}
