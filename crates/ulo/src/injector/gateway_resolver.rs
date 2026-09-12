use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::error::SetupResult;

use crate::traits_helpers::{WsErrorHandlerArc, WsGuardEntry, WsInterceptorEntry};
use crate::websocket::{Gateway, GatewayWrapper};

use super::Container;

pub struct GatewayResolver {
    container: Rc<RefCell<Container>>,
}

impl GatewayResolver {
    pub fn new(container: Rc<RefCell<Container>>) -> Self {
        Self { container }
    }

    pub fn resolve(&self) -> SetupResult<HashMap<String, Arc<GatewayWrapper>>> {
        let raw = self.container.borrow().get_gateways().clone();
        raw.into_iter()
            .map(|(path, gateway)| {
                let wrapper = self.wrap_gateway(gateway)?;
                Ok((path, Arc::new(wrapper)))
            })
            .collect()
    }

    fn wrap_gateway(&self, gateway: Arc<Box<dyn Gateway>>) -> SetupResult<GatewayWrapper> {
        let enhancers = gateway.enhancers();
        let guards = self.resolve_guards(enhancers.guard_tokens)?;
        let interceptors = self.resolve_interceptors(enhancers.interceptor_tokens)?;
        let error_handlers = self.resolve_error_handlers(enhancers.error_handler_tokens)?;
        let metadata = gateway.metadata();
        let handler_metadata: HashMap<String, std::sync::Arc<crate::context::Metadata>> =
            gateway.handler_metadata().into_iter().collect();

        let mut handler_guards: HashMap<String, Vec<WsGuardEntry>> = HashMap::new();
        let mut handler_interceptors: HashMap<String, Vec<WsInterceptorEntry>> = HashMap::new();
        let mut handler_error_handlers: HashMap<String, Vec<WsErrorHandlerArc>> = HashMap::new();

        for handler in enhancers.handlers {
            let event = handler.event;
            handler_guards.insert(
                event.clone(),
                self.resolve_tokens_only(handler.guard_tokens)?,
            );
            handler_interceptors.insert(
                event.clone(),
                self.resolve_interceptor_tokens_only(handler.interceptor_tokens)?,
            );
            handler_error_handlers.insert(
                event.clone(),
                self.resolve_error_handler_tokens_only(handler.error_handler_tokens)?,
            );
        }

        Ok(GatewayWrapper::new(
            gateway,
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

    fn resolve_guards(&self, tokens: Vec<String>) -> SetupResult<Vec<WsGuardEntry>> {
        let mut guards = self.container.borrow().get_global_ws_guards();
        for token in tokens {
            let entry = self.resolve_guard_by_token(&token)?;
            guards.push(entry);
        }
        Ok(guards)
    }

    fn resolve_interceptors(&self, tokens: Vec<String>) -> SetupResult<Vec<WsInterceptorEntry>> {
        let mut interceptors = self.container.borrow().get_global_ws_interceptors();
        for token in tokens {
            let entry = self.resolve_interceptor_by_token(&token)?;
            interceptors.push(entry);
        }
        Ok(interceptors)
    }

    fn resolve_error_handlers(&self, tokens: Vec<String>) -> SetupResult<Vec<WsErrorHandlerArc>> {
        let mut error_handlers = self.container.borrow().get_global_ws_error_handlers();
        for token in tokens {
            error_handlers.push(self.resolve_error_handler_by_token(&token)?);
        }
        Ok(error_handlers)
    }

    fn resolve_tokens_only(&self, tokens: Vec<String>) -> SetupResult<Vec<WsGuardEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_guard_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_interceptor_tokens_only(
        &self,
        tokens: Vec<String>,
    ) -> SetupResult<Vec<WsInterceptorEntry>> {
        tokens
            .into_iter()
            .map(|token| {
                let entry = self.resolve_interceptor_by_token(&token)?;
                Ok(entry)
            })
            .collect()
    }

    fn resolve_error_handler_tokens_only(
        &self,
        tokens: Vec<String>,
    ) -> SetupResult<Vec<WsErrorHandlerArc>> {
        tokens
            .into_iter()
            .map(|t| self.resolve_error_handler_by_token(&t))
            .collect()
    }

    fn resolve_guard_by_token(&self, token: &str) -> SetupResult<WsGuardEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_guards
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "WS Guard '{}' not found in registry. A guard registers automatically by \
                     implementing Guard<WsContext>; make sure the provider is in the module's \
                     `providers` list. For `provider_factory!` under a string/const token, name \
                     the produced type so it can be detected — annotate the closure's return type \
                     (`|| -> MyGuard`) or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_interceptor_by_token(&self, token: &str) -> SetupResult<WsInterceptorEntry> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_interceptors
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "WS Interceptor '{}' not found in registry. An interceptor registers \
                     automatically by implementing Interceptor<WsContext>; make sure the provider \
                     is in the module's `providers` list. For `provider_factory!` under a \
                     string/const token, name the produced type so it can be detected — annotate \
                     the closure's return type (`|| -> MyInterceptor`) or pass a type hint.",
                    token
                )
                .into()
            })
    }

    fn resolve_error_handler_by_token(&self, token: &str) -> SetupResult<WsErrorHandlerArc> {
        self.container
            .borrow()
            .get_role_registry()
            .ws_error_handlers
            .get(token)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "WS ErrorHandler '{}' not found in registry. An error handler registers \
                     automatically by implementing ErrorHandler<WsContext, WsMessage>; make sure \
                     the provider is in the module's `providers` list. For `provider_factory!` \
                     under a string/const token, name the produced type so it can be detected — \
                     annotate the closure's return type or pass a type hint.",
                    token
                )
                .into()
            })
    }
}
