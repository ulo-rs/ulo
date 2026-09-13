//! Everything that is WebSocket and nothing that is not: the connection, its session, the
//! messages on it, the gateway that answers them, the broadcast surface, the context one
//! execution runs in, and the adapter trait an integration crate implements.
//!
//! What a gateway shares with the other transports — `Guard`, `Interceptor`, `FromContext`,
//! `Payload` — is in the crate's core, because it means the same thing there.

mod adapter;
mod broadcast;
mod broadcast_module;
mod broadcast_provider;
mod context;
mod extractors;
mod gateway;
mod gateway_wrapper;
pub mod helpers;
mod lifecycle;
mod session;
mod ws_client;
mod ws_client_map;
mod ws_error;
mod ws_handler_output;
mod ws_message;

pub use self::context::WsContext;
pub use adapter::{MessageCallbackResult, WsAdapter, WsConnectionCallbacks};
pub use broadcast::{
    BroadcastError, BroadcastService, BroadcastTarget, ClientId, RoomId, SendError, TrySendError,
    WsSink,
};
pub use broadcast_module::BroadcastModule;
pub use extractors::PayloadError;
pub use gateway::{Gateway, GatewayEnhancers, GatewayHandlerEnhancers};
pub(crate) use gateway_wrapper::GatewayWrapper;
pub use lifecycle::WsLifecycleHandle;
pub use session::Session;
pub use ws_client::{WsClient, WsHandshake};
pub(crate) use ws_client_map::WsClientMap;
pub use ws_error::{DisconnectReason, WsError, close_code, refusal_frames};
pub use ws_handler_output::WsHandlerOutput;
pub use ws_message::{CloseFrame, WsMessage};

/// Convenience alias for the return type of `#[subscribe_message]` handlers.
pub type WsHandlerResult = Result<WsHandlerOutput, WsError>;
