use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::SetupResult;

use crate::adapter::{GrpcServiceSource, ResolvedGrpcEnhancers};
use crate::traits_helpers::{GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry};

use super::UloContainer;

/// Resolves one gRPC service's enhancer bundle from the role registry by token.
/// Mirrors [`RpcControllerResolver`](super::RpcControllerResolver) — called by the instance
/// loader while services are stored, so a misdeclared token fails `create()`. Bind hands the
/// stored `(service, enhancers)` pair to the adapter, which forwards `enhancers` into
/// [`GrpcServiceSource::register_with`].
pub struct GrpcServiceResolver {
    container: Rc<RefCell<UloContainer>>,
}

impl GrpcServiceResolver {
    pub fn new(container: Rc<RefCell<UloContainer>>) -> Self {
        Self { container }
    }

    pub(crate) fn resolve_for(
        &self,
        svc: &dyn GrpcServiceSource,
    ) -> SetupResult<ResolvedGrpcEnhancers> {
        let enhancers = svc.enhancers();
        let guards = self.resolve_guards(enhancers.guard_tokens)?;
        let interceptors = self.resolve_interceptors(enhancers.interceptor_tokens)?;
        let error_handlers = self.resolve_error_handlers(enhancers.error_handler_tokens)?;

        let mut handler_guards: HashMap<String, Vec<GrpcGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<GrpcInterceptorEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<GrpcErrorHandlerArc>> = HashMap::new();
        for handler in enhancers.handlers {
            let method = handler.method;
            handler_guards.insert(method.clone(), self.resolve_guards(handler.guard_tokens)?);
            handler_interceptors.insert(
                method.clone(),
                self.resolve_interceptors(handler.interceptor_tokens)?,
            );
            handler_error_handlers.insert(
                method,
                self.resolve_error_handlers(handler.error_handler_tokens)?,
            );
        }

        Ok(ResolvedGrpcEnhancers {
            guards,
            handler_guards,
            interceptors,
            handler_interceptors,
            error_handlers,
            handler_error_handlers,
        })
    }

    fn resolve_guards(&self, tokens: Vec<String>) -> SetupResult<Vec<GrpcGuardEntry>> {
        let mut guards = self.container.borrow().get_global_grpc_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_guard_by_token(&self, token: &str) -> SetupResult<GrpcGuardEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .grpc_guards
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "gRPC Guard '{}' not found in registry. A guard registers automatically by \
                     implementing Guard<GrpcContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyGuard`) or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> SetupResult<Vec<GrpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_grpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> SetupResult<GrpcInterceptorEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .grpc_interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "gRPC Interceptor '{}' not found in registry. An interceptor registers \
                     automatically by implementing Interceptor<GrpcContext>; make sure the \
                     provider is in the module's `providers` list. For `provider_factory!` under a \
                     string/const token, name the produced type so it can be detected — annotate \
                     the closure's return type (`|| -> MyInterceptor`) or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> SetupResult<Vec<GrpcErrorHandlerArc>> {
        let mut handlers = self.container.borrow().get_global_grpc_error_handlers();
        for token in tokens {
            handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(handlers)
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> SetupResult<GrpcErrorHandlerArc> {
        self.container
            .borrow()
            .get_role_registry()
            .grpc_error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "gRPC ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<GrpcContext, GrpcStatus>; make \
                     sure the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                ).into()
            })
    }
}
