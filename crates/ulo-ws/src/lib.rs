//! The WebSocket transport (transports DESIGN §4): gateways, sessions, the connection hooks, the
//! message envelope, rooms and broadcast.
//!
//! ```ignore
//! # // reason: names a session type, a guard and message types the caller defines
//! #[module(imports = [ulo_ws::WsModule::for_root()], controllers = [ChatGateway])]
//! pub struct ChatModule;
//!
//! #[routes]
//! #[ulo_ws::gateway(path = "/chat", namespace = "lobby", session = ChatSession, connect_guards(TokenGuard))]
//! impl ChatGateway {
//!     #[ulo_ws::message("chat.send")]
//!     async fn send(&self, msg: Payload<ChatMessage>, rooms: Dep<Rooms>) -> Result<Ack, ChatError> { /* .. */ }
//! }
//! ```
//!
//! Two transports: [`Ws`] covers message handlers, one execution per message, and [`WsConnect`]
//! the connection phase, one execution per connection, whose handler `ulo-ws` mounts itself with
//! the gateway's connect guards. A gateway on the HTTP server's port is reached through the
//! upgrade hand-off [`WsModule`] registers; one declared `port = own` through a standalone server,
//! `ulo_ws_hyper::Server`. Both serve through a [`GatewayTable`]: its handshake decision answers
//! each upgrade request, and [`Switch::serve`] drives each connection on the upgraded stream, so
//! both run the same steps at the same moments. Every task either starts runs on the app's
//! runtime: the crate depends on no runtime and owns no socket.
//!
//! `#[message]` builds a [`WsHandler`] for each handler and `#[gateway]` mounts the connect handler
//! through [`GatewayHooks`]; a gateway written without them builds the same values.

mod broadcast;
mod codec;
mod connection;
mod envelope;
mod gateway;
mod handler;
mod handoff;
mod module;
mod rooms;
mod session;
mod table;
mod trace;
mod transport;

/// The types [`WsModule`]'s and [`GatewaySettings`]' builders take that another `ulo` crate
/// defines, re-exported so an application configuring WebSocket imports them from one place, as
/// `ulo_http::config` does for HTTP. Each name is the defining crate's type, not a copy.
pub mod config {
    #[doc(no_inline)]
    pub use ulo::Bound;
    #[doc(no_inline)]
    pub use ulo_transport::Count;
}

#[doc(hidden)]
pub mod __private;

pub use broadcast::{Audience, BroadcastAdapter, InMemory, NodeId, Target};
pub use codec::Codec;
pub use connection::{ConnId, Connection, Handshake, Refusal, Switch};
pub use envelope::{Frame, MessageId, Payload};
pub use gateway::{
    AfterInit, CloseCodeError, ConnectRefused, DisconnectReason, Gateway, GatewayConfig, GatewayRef, GatewaySettings,
    OnConnect, OnDisconnect, Overflow, Port, Refuse,
};
pub use handler::{GatewayHooks, WsHandler};
pub use module::WsModule;
pub use rooms::{Broadcast, BroadcastError, Rooms, RoomsIn};
pub use session::{Session, SessionFactory, SessionHandle};
#[cfg(feature = "conformance-probe")]
pub use table::{AnswerHold, MessagesRead};
pub use table::{GatewayDefaults, GatewayTable};
pub use transport::{ConnectCx, ConnectReply, ConnectionInfo, NoHandler, Reply, UpgradeHead, Ws, WsConnect, WsCx};
pub use bytes::Bytes;
pub use http;
pub use ulo_transport::{
    CallError, Classify, Detail, Details, ErrorKind, ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Valid, Validate,
};
pub use ulo_ws_macros::{gateway, message};
