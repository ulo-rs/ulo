//! The lifecycle handle for the HTTP adapter.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use async_trait::async_trait;

use crate::error::AdapterResult;
use crate::server_lifecycle::{ServerLifecycle, ShutdownCallback};

/// Lifecycle handle for an HTTP adapter. Constructed by each adapter
/// crate's `into_lifecycle` implementation; owns the concrete state
/// needed to serve and shut down. The orchestrator only sees the
/// `ServerLifecycle` surface.
pub struct HttpLifecycleHandle {
    local_addr: SocketAddr,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl HttpLifecycleHandle {
    /// Build a handle from the local address, the long-running serve
    /// future, and a callback that triggers graceful shutdown.
    pub fn new<F, Fut>(
        local_addr: SocketAddr,
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
impl ServerLifecycle for HttpLifecycleHandle {
    fn name(&self) -> &'static str {
        "http"
    }

    fn local_addr(&self) -> Option<SocketAddr> {
        Some(self.local_addr)
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
