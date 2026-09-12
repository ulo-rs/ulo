//! The lifecycle handle for the WebSocket adapter.
//!
//! One handle per unique separate-port listener. A single adapter produces N handles inside
//! `WebSocketAdapter::into_lifecycle_handles`; each handle gets a clone of the adapter's shutdown
//! signal in its callback, so calling `shutdown` on any handle flips the watch and every port wakes
//! up to drain. Idempotent by construction — `watch::Sender::send(true)` after the value is already
//! `true` is a no-op.
//!
//! The handle owns the concrete adapter and exposes only [`ServerLifecycle`] to the orchestrator,
//! which cannot tell one transport from another once the handle is boxed.

use std::net::SocketAddr;
use std::pin::Pin;

use async_trait::async_trait;

use crate::adapter::server_lifecycle::{ServerLifecycle, ShutdownCallback};
use crate::error::AdapterResult;

pub struct WsLifecycleHandle {
    local_addr: SocketAddr,
    serve: Option<Pin<Box<dyn Future<Output = ()> + Send + 'static>>>,
    shutdown: Option<ShutdownCallback>,
}

impl WsLifecycleHandle {
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

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}

#[async_trait]
impl ServerLifecycle for WsLifecycleHandle {
    fn name(&self) -> &'static str {
        "websocket"
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

// ─── RPC ─────────────────────────────────────────────────────────────────────
