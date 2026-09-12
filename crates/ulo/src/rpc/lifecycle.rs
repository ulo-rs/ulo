//! The lifecycle handle for the RPC adapter.
//!
//! Owns the concrete adapter and exposes only [`ServerLifecycle`] to the orchestrator, which
//! cannot tell one transport from another once the handle is boxed. A subject-based transport
//! has no address to report, which is why `local_addr` is optional here and not on the others.

use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;

use async_trait::async_trait;

use crate::adapter::server_lifecycle::{ServerLifecycle, ShutdownCallback};
use crate::error::AdapterResult;

pub struct RpcLifecycleHandle {
    local_addr: Option<SocketAddr>,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl RpcLifecycleHandle {
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
impl ServerLifecycle for RpcLifecycleHandle {
    fn name(&self) -> &'static str {
        "rpc"
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
