//! The RPC attributes, re-exported by `ulo-rpc`: `#[message("pattern")]` and `#[event("pattern")]`
//! on the methods of a `#[handlers]` impl, each a thin layer over `ulo-handler-codegen`.
//!
//! ```ignore
//! # // reason: a proc-macro crate cannot depend on the crates the expansion names
//! #[handlers]
//! impl InvoicesController {
//!     #[ulo_rpc::message("invoices.create")]
//!     async fn create(&self, req: Payload<NewInvoice>) -> Result<Invoice, BillingError> { /* .. */ }
//!
//!     #[ulo_rpc::event("user.created")]
//!     async fn on_user(&self, evt: Payload<UserCreated>) -> Result<(), BillingError> { /* .. */ }
//!
//!     #[ulo_rpc::event("prices.changed", every_instance)]
//!     async fn on_prices(&self, evt: Payload<PricesChanged>) -> Result<(), BillingError> { /* .. */ }
//! }
//! ```
//!
//! The call shape is read from the signature, an `Inbound<T>` parameter for a streamed request and
//! a stream return for a streamed reply, and recorded with `HandlerSpec::shape`.

mod event;
mod message;

use proc_macro::TokenStream;

/// A request-reply handler for `pattern`, in any of the four call shapes.
#[proc_macro_attribute]
pub fn message(attr: TokenStream, item: TokenStream) -> TokenStream {
    message::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A fire-and-forget handler for `pattern`, delivered to one of the server instances listening on
/// it; `#[event("pattern", every_instance)]` delivers it to each of them
/// (`ulo_rpc::RpcHandler::every_instance`).
#[proc_macro_attribute]
pub fn event(attr: TokenStream, item: TokenStream) -> TokenStream {
    event::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
