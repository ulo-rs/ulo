use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::dispatch::transport::Rpc;
use crate::error::SetupResult;
use crate::rpc::{RpcControllerSource, RpcControllerWrapper, RpcHandlerEnhancers};

use super::{Declared, resolve_handler, resolve_target};
use crate::di::internal::Container;

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
        source: Arc<dyn RpcControllerSource>,
    ) -> SetupResult<RpcControllerWrapper> {
        let declared = source.enhancers();
        let metadata = source.metadata();
        let handler_metadata: HashMap<String, Arc<crate::context::Metadata>> =
            source.handler_metadata().into_iter().collect();

        let container = self.container.borrow();
        let registry = &container.role_registry().rpc;

        let controller = resolve_target::<Rpc>(
            registry,
            &container.global_rpc,
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
            let RpcHandlerEnhancers { pattern, .. } = &handler;
            let pattern = pattern.clone();
            let set = resolve_handler::<Rpc>(
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
            handler_guards.insert(pattern.clone(), set.guards);
            handler_interceptors.insert(pattern.clone(), set.interceptors);
            handler_error_handlers.insert(pattern, set.error_handlers);
        }

        Ok(RpcControllerWrapper::new(
            source,
            controller.guards,
            controller.interceptors,
            controller.error_handlers,
            metadata,
            handler_metadata,
            handler_guards,
            handler_interceptors,
            handler_error_handlers,
        ))
    }
}
