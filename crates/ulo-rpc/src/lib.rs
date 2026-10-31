//! The RPC transport (transports DESIGN §5): the [`Rpc`] marker and its context [`RpcCx`], one
//! frame grammar with JSON and CBOR codecs, pattern routing, all four call shapes, the
//! [`RpcClient`], and the [`link`] SPI each transport crate implements once.
//!
//! ```no_run
//! # use ulo::{BoxError, Guard, injectable, routes};
//! # use ulo_rpc::{CallHeaders, Payload, Rpc, RpcCx};
//! # use ulo_transport::{CallError, Classify};
//! # #[derive(serde::Deserialize)] struct NewInvoice;
//! # #[derive(serde::Serialize)] struct Invoice;
//! # #[derive(serde::Deserialize)] struct UserCreated;
//! # #[derive(Debug, Classify)] enum BillingError { #[classify(internal)] Failed }
//! # impl std::fmt::Display for BillingError { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("failed") } }
//! # impl std::error::Error for BillingError {}
//! # struct ServiceTokenGuard;
//! # impl Guard<Rpc> for ServiceTokenGuard { async fn can_activate(&self, _cx: &RpcCx) -> Result<bool, BoxError> { Ok(true) } }
//! #[injectable]
//! struct InvoicesController;
//!
//! #[routes]
//! #[guards(value = ServiceTokenGuard)]
//! impl InvoicesController {
//!     #[ulo_rpc::message("invoices.create")]
//!     async fn create(&self, req: Payload<NewInvoice>, h: CallHeaders) -> Result<Invoice, BillingError> { Ok(Invoice) }
//!
//!     #[ulo_rpc::event("user.created")]
//!     async fn on_user(&self, evt: Payload<UserCreated>) -> Result<(), BillingError> { Ok(()) }
//! }
//!
//! // then: app.bind(ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("0.0.0.0:7000")))
//! ```
//!
//! The call shape comes from the signature: an [`Inbound<T>`] parameter is a streamed request, a
//! stream return a streamed reply. The attributes build an [`RpcHandler`] for each handler, and a
//! controller written without them builds the same value in its `Controller::mount`.

mod client;
mod client_module;
mod codec;
mod dispatch;
mod extract;
mod frame;
mod handler;
mod held;
mod server;
mod trace;
mod transport;

pub mod link;

/// The types [`Server`]'s and [`RpcClientModule`]'s builders take that another `ulo` crate defines,
/// re-exported so an application configuring RPC imports them from one place, as `ulo_http::config`
/// does for HTTP. Each name is the defining crate's type, not a copy.
pub mod config {
    #[doc(no_inline)]
    pub use ulo::Bound;
    #[doc(no_inline)]
    pub use ulo_transport::Count;
}

#[doc(hidden)]
pub mod __private;

pub use client::{Call, Emit, RpcClient, RpcError, RpcStream, StreamCall, UnusableLink, ZeroTimeout};
pub use client_module::RpcClientModule;
pub use codec::Codec;
pub use extract::{Inbound, Payload};
pub use frame::{Data, ErrorBody, Frame, PayloadKind};
pub use handler::RpcHandler;
pub use link::{
    Ack, Capabilities, Delivery, DeliveryMode, FrameTooLarge, FrameUnencodable, Link, Misconfigured, NoDestination, NoTimer, Ordering,
    Outbound, Pattern, ReplyPath, ReplyTo,
};
pub use server::Server;
pub use transport::{CallHeaders, LinkInfo, NoHandler, Reply, Rpc, RpcCx};
pub use ulo_rpc_macros::{event, message};
pub use bytes::Bytes;
pub use ulo_transport::{
    CallError, Classify, Detail, Details, ErrorKind, ExtractError, FieldViolation, FromCall, IntoReply, IntoReplyError, Valid,
    Validate,
};
