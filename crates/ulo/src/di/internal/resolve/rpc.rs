use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::error::SetupResult;

use super::super::Container;
use crate::enhancer::{Guard, Interceptor};
use crate::rpc::{RpcContext, RpcControllerSource, RpcControllerWrapper, RpcHandlerResult};
use crate::spi::{RpcErrorHandlerArc, RpcGuardEntry, RpcInterceptorEntry};
/// Resolves one RPC controller's enhancer tokens into a ready-to-serve
/// `RpcControllerWrapper`. Called by the instance loader while controllers are stored, so a
/// misdeclared token fails `create()`; bind hands the stored wrapper to the adapter.
pub(crate) struct RpcControllerResolver {
    container: Rc<RefCell<Container>>,
}

impl RpcControllerResolver {
    pub(crate) fn new(container: Rc<RefCell<Container>>) -> Self {
        Self { container }
    }

    pub(crate) fn wrap_controller(
        &self,
        source: std::sync::Arc<dyn RpcControllerSource>,
    ) -> SetupResult<RpcControllerWrapper> {
        let enhancers = source.enhancers();
        let guards = self.resolve_guards(enhancers.guard_tokens, enhancers.guards)?;
        let interceptors =
            self.resolve_interceptors(enhancers.interceptor_tokens, enhancers.interceptors)?;
        let error_handlers =
            self.resolve_error_handlers(enhancers.error_handler_tokens, enhancers.error_handlers)?;
        let metadata = source.metadata();
        let handler_metadata: HashMap<String, std::sync::Arc<crate::context::Metadata>> =
            source.handler_metadata().into_iter().collect();

        let mut handler_guards: HashMap<String, Vec<RpcGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<RpcInterceptorEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<RpcErrorHandlerArc>> = HashMap::new();

        for handler in enhancers.handlers {
            let pattern = handler.pattern;
            handler_guards.insert(
                pattern.clone(),
                self.resolve_handler_guards(handler.guard_tokens, handler.guards)?,
            );
            handler_interceptors.insert(
                pattern.clone(),
                self.resolve_handler_interceptors(
                    handler.interceptor_tokens,
                    handler.interceptors,
                )?,
            );
            handler_error_handlers.insert(
                pattern.clone(),
                self.resolve_handler_error_handlers(
                    handler.error_handler_tokens,
                    handler.error_handlers,
                )?,
            );
        }

        Ok(RpcControllerWrapper::new(
            source,
            guards,
            interceptors,
            error_handlers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
        ))
    }

    fn resolve_guards(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Guard<RpcContext>>>,
    ) -> SetupResult<Vec<RpcGuardEntry>> {
        let mut guards = self.container.borrow().global_rpc.guards.clone();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        guards.extend(instances.into_iter().map(RpcGuardEntry::Ready));
        Ok(guards)
    }

    fn resolve_interceptors(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>>,
    ) -> SetupResult<Vec<RpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().global_rpc.interceptors.clone();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        interceptors.extend(instances.into_iter().map(RpcInterceptorEntry::Ready));
        Ok(interceptors)
    }

    fn resolve_error_handlers(
        &self,
        tokens: Vec<String>,
        instances: Vec<RpcErrorHandlerArc>,
    ) -> SetupResult<Vec<RpcErrorHandlerArc>> {
        let mut error_handlers = self.container.borrow().global_rpc.error_handlers.clone();
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        error_handlers.extend(instances);
        Ok(error_handlers)
    }

    fn resolve_guard_by_token(&self, token: &str) -> SetupResult<RpcGuardEntry> {
        self.container
            .borrow()
            .role_registry()
            .rpc
            .guards
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "RPC Guard '{}' not found in registry. A guard registers automatically by \
                     implementing Guard<RpcContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyGuard`) or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> SetupResult<RpcInterceptorEntry> {
        self.container
            .borrow()
            .role_registry()
            .rpc
            .interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "RPC Interceptor '{}' not found in registry. An interceptor registers \
                     automatically by implementing Interceptor<RpcContext>; make sure the provider \
                     is in the module's `providers` list. For `provider_factory!` under a \
                     string/const token, name the produced type so it can be detected — annotate \
                     the closure's return type (`|| -> MyInterceptor`) or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> SetupResult<RpcErrorHandlerArc> {
        self.container
            .borrow()
            .role_registry()
            .rpc.error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "RPC ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<RpcContext, RpcHandlerResult>; make sure \
                     the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_handler_guards(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Guard<RpcContext>>>,
    ) -> SetupResult<Vec<RpcGuardEntry>> {
        let mut guards: Vec<RpcGuardEntry> = tokens
            .into_iter()
            .map(|token| self.resolve_guard_by_token(&token))
            .collect::<SetupResult<_>>()?;
        guards.extend(instances.into_iter().map(RpcGuardEntry::Ready));
        Ok(guards)
    }

    fn resolve_handler_interceptors(
        &self,
        tokens: Vec<String>,
        instances: Vec<Arc<dyn Interceptor<RpcContext, RpcHandlerResult>>>,
    ) -> SetupResult<Vec<RpcInterceptorEntry>> {
        let mut interceptors: Vec<RpcInterceptorEntry> = tokens
            .into_iter()
            .map(|token| self.resolve_interceptor_by_token(&token))
            .collect::<SetupResult<_>>()?;
        interceptors.extend(instances.into_iter().map(RpcInterceptorEntry::Ready));
        Ok(interceptors)
    }

    fn resolve_handler_error_handlers(
        &self,
        tokens: Vec<String>,
        instances: Vec<RpcErrorHandlerArc>,
    ) -> SetupResult<Vec<RpcErrorHandlerArc>> {
        let mut error_handlers: Vec<RpcErrorHandlerArc> = tokens
            .into_iter()
            .map(|t| self.resolve_error_handler_by_token(&t))
            .collect::<SetupResult<_>>()?;
        error_handlers.extend(instances);
        Ok(error_handlers)
    }
}
