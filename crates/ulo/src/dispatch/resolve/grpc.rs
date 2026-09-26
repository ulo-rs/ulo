use parking_lot::RwLock;
use std::sync::Arc;

use crate::dispatch::transport::Grpc;
use crate::error::SetupResult;
use crate::grpc::{GrpcServiceSource, ResolvedGrpcEnhancers};

use super::{Declared, resolve};
use crate::di::internal::Container;

/// Resolves one gRPC service's enhancer bundle from the role registry by token.
/// Mirrors [`RpcControllerResolver`](super::RpcControllerResolver) — called by the instance
/// loader while services are stored, so a misdeclared token fails `create()`. Bind hands the
/// stored `(service, enhancers)` pair to the adapter, which forwards `enhancers` into
/// [`GrpcServiceSource::register_with`].
pub(crate) struct GrpcServiceResolver {
    container: Arc<RwLock<Container>>,
}

impl GrpcServiceResolver {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
        Self { container }
    }

    pub(crate) fn resolve_for(
        &self,
        svc: &dyn GrpcServiceSource,
    ) -> SetupResult<ResolvedGrpcEnhancers> {
        let declared = svc.enhancers();
        let container = self.container.read();
        let registry = &container.role_registry().grpc;

        let resolved = resolve::<Grpc>(
            registry,
            &container.global_grpc,
            Declared {
                guards: declared.guards,
                interceptors: declared.interceptors,
                error_handlers: declared.error_handlers,
            },
            declared.handlers.into_iter().map(|handler| {
                (
                    handler.method,
                    Declared {
                        guards: handler.guards,
                        interceptors: handler.interceptors,
                        error_handlers: handler.error_handlers,
                    },
                )
            }),
        )?;

        Ok(ResolvedGrpcEnhancers(resolved))
    }
}
