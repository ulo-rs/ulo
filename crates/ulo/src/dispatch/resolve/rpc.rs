use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::dispatch::transport::Rpc;
use crate::error::SetupResult;
use crate::rpc::{RpcControllerSource, RpcControllerWrapper};

use super::{Declared, resolve};
use crate::di::internal::Container;

/// Resolves one RPC controller's enhancer tokens into a ready-to-serve
/// `RpcControllerWrapper`. Called by the instance loader while controllers are stored, so a
/// misdeclared token fails `create()`; bind hands the stored wrapper to the adapter.
pub(crate) struct RpcControllerResolver {
    container: Arc<RwLock<Container>>,
}

impl RpcControllerResolver {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
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

        let container = self.container.read();
        let registry = &container.role_registry().rpc;

        let resolved = resolve::<Rpc>(
            registry,
            &container.global_rpc,
            Declared {
                guards: declared.guards,
                interceptors: declared.interceptors,
                error_handlers: declared.error_handlers,
            },
            declared.handlers.into_iter().map(|handler| {
                (
                    handler.pattern,
                    Declared {
                        guards: handler.guards,
                        interceptors: handler.interceptors,
                        error_handlers: handler.error_handlers,
                    },
                )
            }),
        )?;

        Ok(RpcControllerWrapper::new(
            source,
            resolved,
            metadata,
            handler_metadata,
        ))
    }
}
