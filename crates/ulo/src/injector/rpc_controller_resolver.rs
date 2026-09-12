use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::SetupResult;

use super::Container;
use crate::rpc::{RpcControllerSource, RpcControllerWrapper};
use crate::traits::{RpcErrorHandlerArc, RpcGuardEntry, RpcInterceptorEntry};

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
        let guards = self.resolve_guards(enhancers.guard_tokens)?;
        let interceptors = self.resolve_interceptors(enhancers.interceptor_tokens)?;
        let error_handlers = self.resolve_error_handlers(enhancers.error_handler_tokens)?;
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
                self.resolve_handler_guards(handler.guard_tokens)?,
            );
            handler_interceptors.insert(
                pattern.clone(),
                self.resolve_handler_interceptors(handler.interceptor_tokens)?,
            );
            handler_error_handlers.insert(
                pattern.clone(),
                self.resolve_handler_error_handlers(handler.error_handler_tokens)?,
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

    fn resolve_guards(&self, tokens: Vec<String>) -> SetupResult<Vec<RpcGuardEntry>> {
        let mut guards = self.container.borrow().global_rpc_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> SetupResult<Vec<RpcInterceptorEntry>> {
        let mut interceptors = self.container.borrow().global_rpc_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> SetupResult<Vec<RpcErrorHandlerArc>> {
        let mut error_handlers = self.container.borrow().global_rpc_error_handlers();
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(error_handlers)
    }

    fn resolve_guard_by_token(&self, token: &str) -> SetupResult<RpcGuardEntry> {
        self.container
            .borrow()
            .role_registry()
            .rpc_guards
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
            .rpc_interceptors
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
            .rpc_error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "RPC ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<RpcContext, RpcData>; make sure \
                     the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_handler_guards(&self, tokens: Vec<String>) -> SetupResult<Vec<RpcGuardEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_guard_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_handler_interceptors(
        &self,
        tokens: Vec<String>,
    ) -> SetupResult<Vec<RpcInterceptorEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_interceptor_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_handler_error_handlers(
        &self,
        tokens: Vec<String>,
    ) -> SetupResult<Vec<RpcErrorHandlerArc>> {
        tokens
            .into_iter()
            .map(|t| self.resolve_error_handler_by_token(&t))
            .collect()
    }
}
