use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::error::SetupResult;
use crate::spi::transport::Ws;
use crate::ws::{Gateway, GatewayWrapper};

use super::super::Container;
use super::enhancers::{Declared, resolve_handler, resolve_target};

pub(crate) struct GatewayResolver {
    container: Rc<RefCell<Container>>,
}

impl GatewayResolver {
    pub(crate) fn new(container: Rc<RefCell<Container>>) -> Self {
        Self { container }
    }

    pub(crate) fn resolve(&self) -> SetupResult<HashMap<String, Arc<GatewayWrapper>>> {
        let raw = self.container.borrow().gateways().clone();
        raw.into_iter()
            .map(|(path, gateway)| {
                let wrapper = self.wrap_gateway(gateway)?;
                Ok((path, Arc::new(wrapper)))
            })
            .collect()
    }

    fn wrap_gateway(&self, gateway: Arc<Box<dyn Gateway>>) -> SetupResult<GatewayWrapper> {
        let declared = gateway.enhancers();
        let metadata = gateway.metadata();
        let handler_metadata: HashMap<String, Arc<crate::context::Metadata>> =
            gateway.handler_metadata().into_iter().collect();

        let container = self.container.borrow();
        let registry = &container.role_registry().ws;

        let gateway_level = resolve_target::<Ws>(
            registry,
            &container.global_ws,
            Declared {
                guard_tokens: declared.guard_tokens,
                guards: declared.guards,
                interceptor_tokens: declared.interceptor_tokens,
                interceptors: declared.interceptors,
                error_handler_tokens: declared.error_handler_tokens,
                error_handlers: declared.error_handlers,
            },
        )?;

        let mut handler_guards = HashMap::new();
        let mut handler_interceptors = HashMap::new();
        let mut handler_error_handlers = HashMap::new();
        for handler in declared.handlers {
            let event = handler.event.clone();
            let set = resolve_handler::<Ws>(
                registry,
                Declared {
                    guard_tokens: handler.guard_tokens,
                    guards: handler.guards,
                    interceptor_tokens: handler.interceptor_tokens,
                    interceptors: handler.interceptors,
                    error_handler_tokens: handler.error_handler_tokens,
                    error_handlers: handler.error_handlers,
                },
            )?;
            handler_guards.insert(event.clone(), set.guards);
            handler_interceptors.insert(event.clone(), set.interceptors);
            handler_error_handlers.insert(event, set.error_handlers);
        }

        Ok(GatewayWrapper::new(
            gateway,
            gateway_level.guards,
            gateway_level.interceptors,
            gateway_level.error_handlers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
        ))
    }
}
