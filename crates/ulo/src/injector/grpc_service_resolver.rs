use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::error::SetupResult;

use super::Container;
use crate::enhancer::{Guard, Interceptor};
use crate::grpc::{GrpcContext, GrpcHandlerResult, GrpcServiceSource, ResolvedGrpcEnhancers};
use crate::spi::{GrpcErrorHandlerArc, GrpcGuardEntry, GrpcInterceptorEntry};
/// Resolves one gRPC service's enhancer bundle from the role registry by token.
/// Mirrors [`RpcControllerResolver`](super::RpcControllerResolver) — called by the instance
/// loader while services are stored, so a misdeclared token fails `create()`. Bind hands the
/// stored `(service, enhancers)` pair to the adapter, which forwards `enhancers` into
/// [`GrpcServiceSource::register_with`].
pub(crate) struct GrpcServiceResolver {
    container: Rc<RefCell<Container>>,
}

impl GrpcServiceResolver {
    pub(crate) fn new(container: Rc<RefCell<Container>>) -> Self {
        Self { container }
    }

    pub(crate) fn resolve_for(
        &self,
        svc: &dyn GrpcServiceSource,
    ) -> SetupResult<ResolvedGrpcEnhancers> {
        let enhancers = svc.enhancers();
        let guards = self.resolve_guards(enhancers.guard_tokens, enhancers.guards)?;
        let interceptors =
            self.resolve_interceptors(enhancers.interceptor_tokens, enhancers.interceptors)?;
        let error_handlers =
            self.resolve_error_handlers(enhancers.error_handler_tokens, enhancers.error_handlers)?;

        let mut handler_guards: HashMap<String, Vec<GrpcGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<GrpcInterceptorEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<GrpcErrorHandlerArc>> = HashMap::new();
        for handler in enhancers.handlers {
            let method = handler.method;
            handler_guards.insert(
                method.clone(),
                self.resolve_handler_guards(handler.guard_tokens, handler.guards)?,
            );
            handler_interceptors.insert(
                method.clone(),
                self.resolve_handler_interceptors(
                    handler.interceptor_tokens,
                    handler.interceptors,
                )?,
            );
            handler_error_handlers.insert(
                method,
                self.resolve_handler_error_handlers(
                    handler.error_handler_tokens,
                    handler.error_handlers,
                )?,
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

    /// Service-level entries, with the transport's globals ahead of them.
    ///
    /// The globals belong to this level alone. A method's own entries stack on top of what is
    /// resolved here, so resolving them with the globals too would run each global twice.
    fn resolve_guards(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Guard<GrpcContext>>>,
    ) -> SetupResult<Vec<GrpcGuardEntry>> {
        let mut guards = self.container.borrow().global_grpc_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        guards.extend(instances.into_iter().map(GrpcGuardEntry::Ready));
        Ok(guards)
    }

    fn resolve_guard_by_token(&self, token: &str) -> SetupResult<GrpcGuardEntry> {
        self.container
            .borrow()
            .role_registry()
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

    fn resolve_interceptors(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>>,
    ) -> SetupResult<Vec<GrpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().global_grpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        interceptors.extend(instances.into_iter().map(GrpcInterceptorEntry::Ready));
        Ok(interceptors)
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> SetupResult<GrpcInterceptorEntry> {
        self.container
            .borrow()
            .role_registry()
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

    fn resolve_error_handlers(
        &self,
        tokens: Vec<String>,
        instances: Vec<GrpcErrorHandlerArc>,
    ) -> SetupResult<Vec<GrpcErrorHandlerArc>> {
        let mut handlers = self.container.borrow().global_grpc_error_handlers();
        for token in tokens {
            handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        handlers.extend(instances);
        Ok(handlers)
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> SetupResult<GrpcErrorHandlerArc> {
        self.container
            .borrow()
            .role_registry()
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

    /// Method-level entries on their own: the globals are already in the service-level vector
    /// these stack on top of.
    fn resolve_handler_guards(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Guard<GrpcContext>>>,
    ) -> SetupResult<Vec<GrpcGuardEntry>> {
        let mut guards: Vec<GrpcGuardEntry> = tokens
            .into_iter()
            .map(|token| self.resolve_guard_by_token(&token))
            .collect::<SetupResult<_>>()?;
        guards.extend(instances.into_iter().map(GrpcGuardEntry::Ready));
        Ok(guards)
    }

    fn resolve_handler_interceptors(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Interceptor<GrpcContext, GrpcHandlerResult>>>,
    ) -> SetupResult<Vec<GrpcInterceptorEntry>> {
        let mut interceptors: Vec<GrpcInterceptorEntry> = tokens
            .into_iter()
            .map(|token| self.resolve_interceptor_by_token(&token))
            .collect::<SetupResult<_>>()?;
        interceptors.extend(instances.into_iter().map(GrpcInterceptorEntry::Ready));
        Ok(interceptors)
    }

    fn resolve_handler_error_handlers(
        &self,
        tokens: Vec<String>,
        instances: Vec<GrpcErrorHandlerArc>,
    ) -> SetupResult<Vec<GrpcErrorHandlerArc>> {
        let mut handlers: Vec<GrpcErrorHandlerArc> = tokens
            .into_iter()
            .map(|token| self.resolve_error_handler_by_token(&token))
            .collect::<SetupResult<_>>()?;
        handlers.extend(instances);
        Ok(handlers)
    }
}
