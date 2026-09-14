//! The lifecycle handle for the gRPC adapter.
//!
//! Owns the concrete adapter and exposes only [`ServerLifecycle`] to the orchestrator, which
//! cannot tell one transport from another once the handle is boxed.

use std::net::SocketAddr;
use std::pin::Pin;

use async_trait::async_trait;

use crate::error::AdapterResult;
use crate::server_lifecycle::{ServerLifecycle, ShutdownCallback};

pub struct GrpcLifecycleHandle {
    local_addr: Option<SocketAddr>,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl GrpcLifecycleHandle {
    pub fn new<F, Fut>(
        local_addr: Option<SocketAddr>,
        serve: Pin<Box<dyn Future<Output = ()> + Send + 'static>>,
        shutdown: F,
    ) -> Self
    where
        F: FnOnce() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AdapterResult> + Send + 'static,
    {
        Self {
            local_addr,
            serve: Some(serve),
            shutdown: Some(Box::new(move || Box::pin(shutdown()))),
        }
    }
}

#[async_trait]
impl ServerLifecycle for GrpcLifecycleHandle {
    fn name(&self) -> &'static str {
        "grpc"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        self.local_addr
    }

    fn take_serve(&mut self) -> Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>> {
        self.serve.take()
    }

    async fn shutdown(&mut self) -> AdapterResult {
        if let Some(cb) = self.shutdown.take() {
            cb().await
        } else {
            Ok(())
        }
    }
}
