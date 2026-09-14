use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::SetupResult;
use crate::grpc::{GrpcServiceSource, ResolvedGrpcEnhancers};
use crate::spi::transport::Grpc;

use super::super::Container;
use super::enhancers::{Declared, resolve_handler, resolve_target};

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
        let declared = svc.enhancers();
        let container = self.container.borrow();
        let registry = &container.role_registry().grpc;

        let service = resolve_target::<Grpc>(
            registry,
            &container.global_grpc,
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
            let method = handler.method.clone();
            let set = resolve_handler::<Grpc>(
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
            handler_guards.insert(method.clone(), set.guards);
            handler_interceptors.insert(method.clone(), set.interceptors);
            handler_error_handlers.insert(method, set.error_handlers);
        }

        Ok(ResolvedGrpcEnhancers {
            guards: service.guards,
            handler_guards,
            interceptors: service.interceptors,
            handler_interceptors,
            error_handlers: service.error_handlers,
            handler_error_handlers,
        })
    }
}
