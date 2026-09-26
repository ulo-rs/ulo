use std::sync::Arc;

use async_trait::async_trait;

use crate::context::Metadata;
use crate::dispatch::{ExecutionResult, Ws};
use crate::enhancer::{ErrorHandlerDeclaration, GuardDeclaration, InterceptorDeclaration};
use crate::ws::WsContext;

use super::{DisconnectReason, WsClient, WsError, WsHandlerOutput};

/// What a gateway declares, read once at registration. Gateway-level entries apply to every
/// handler; each `handlers` entry adds to one event. A flat descriptor instead of a dozen accessor
/// methods — the gateway macro builds it, the resolver reads it once.
///
/// Each role is one vector in the order written, and the resolver keeps that order. A guard or
/// interceptor entry is a token, a value or a constructor (see
/// [`GuardDeclaration`](crate::enhancer::GuardDeclaration)); an error-handler entry is a token or a
/// value.
#[derive(Default)]
pub struct GatewayEnhancers {
    pub guards: Vec<GuardDeclaration<Ws>>,
    pub interceptors: Vec<InterceptorDeclaration<Ws>>,
    pub error_handlers: Vec<ErrorHandlerDeclaration<Ws>>,
    pub handlers: Vec<GatewayHandlerEnhancers>,
}

/// What one handler declares on top of its gateway's, keyed by event. The same spellings as
/// [`GatewayEnhancers`].
#[derive(Default)]
pub struct GatewayHandlerEnhancers {
    pub event: String,
    pub guards: Vec<GuardDeclaration<Ws>>,
    pub interceptors: Vec<InterceptorDeclaration<Ws>>,
    pub error_handlers: Vec<ErrorHandlerDeclaration<Ws>>,
}

/// A WebSocket gateway: it answers a connection's lifecycle and every message on it.
///
/// A gateway is a singleton in `providers:`, not a dispatch target in `controllers:`, so path,
/// namespace and port are read straight off the instance. Declare one with
/// `#[websocket_gateway]` on the struct and `#[subscriptions]` on the handler impl. Implement
/// this trait by hand to override [`event_field`](Gateway::event_field) or to read a
/// disconnect's [`DisconnectReason`] — the macros forward neither.
#[async_trait]
pub trait Gateway: Send + Sync {
    /// The DI token this gateway is registered under.
    fn token(&self) -> String;

    /// The path clients connect to, such as `/chat`.
    fn path(&self) -> String;

    /// The namespace this gateway's clients belong to, if it declares one. A broadcast can
    /// target a single namespace.
    fn namespace(&self) -> Option<String> {
        None
    }

    /// The port this gateway listens on.
    ///
    /// `None` (default) means same port as the HTTP server.
    /// `Some(port)` triggers a separate WebSocket server on that port — requires a
    /// `WsAdapter` to be registered via `UloApplication::use_websocket_adapter()`.
    fn port(&self) -> Option<u16> {
        None
    }

    /// Called once after the gateway path is registered with the adapter, before any connections.
    async fn after_init(&self) {}

    /// Connection lifecycle: called when a client connects
    async fn on_connect(&self, client: &WsClient, context: &WsContext) -> Result<(), WsError> {
        // Default implementation: allow all connections
        let _ = (client, context);
        Ok(())
    }

    /// Connection lifecycle: called when a client disconnects
    /// Connection teardown. `context` is the disconnect's own execution, and is how the connection's
    /// [`Session`](crate::ws::Session) is read one last time. No enhancers run here — a
    /// disconnect cannot be rejected.
    async fn on_disconnect(
        &self,
        client: &WsClient,
        reason: DisconnectReason,
        context: &WsContext,
    ) {
        // Default implementation: no-op
        let _ = (client, reason, context);
    }

    /// The JSON field name used to route incoming messages to a handler.
    ///
    /// Default: `"event"` — matches the standard `{"event":"...", ...}` convention.
    /// Override to `"type"` for graphql-ws protocol compatibility.
    fn event_field(&self) -> &str {
        "event"
    }

    /// Route message to appropriate handler based on event name.
    ///
    /// `Ok(WsHandlerOutput)` for the success path (Empty / One / Many);
    /// `Err` carries the user's typed error so the dispatcher can run the
    /// chain on it before falling back to `WsError::to_message`.
    async fn handle_event(&self, ctx: &WsContext) -> ExecutionResult<WsHandlerOutput, WsError>;

    /// What the gateway declares for every handler, before any handler adds to it.
    fn metadata(&self) -> Arc<Metadata> {
        Arc::new(Metadata::new())
    }

    /// Per-event metadata for handlers that declare their own, already merged over the gateway's.
    /// An event absent from this list reads [`metadata`](Self::metadata).
    fn handler_metadata(&self) -> Vec<(String, Arc<Metadata>)> {
        Vec::new()
    }

    /// All enhancer tokens for this gateway — gateway-level plus per-handler — resolved once at
    /// startup. Default is empty (a gateway with no declared enhancers).
    fn enhancers(&self) -> GatewayEnhancers {
        GatewayEnhancers::default()
    }
}
