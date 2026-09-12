//! What a handler asks for, on any transport.
//!
//! A handler parameter is anything implementing [`FromContext`] for that handler's context, which
//! is what makes an HTTP extractor a compile error in a WebSocket handler rather than something a
//! macro has to catch by name. The extractors here work against more than one context; the ones
//! particular to a transport live with it, under `http::extract`, `grpc::extract`, and the `rpc`
//! and `ws` modules.
//!
//! A body is single-use because it may be a stream. [`take_body`] yields it once and names the
//! second asker; an extractor the handler macro recognises is rejected at compile time, and one it
//! does not fails at extraction with [`BodyExtractionError`], naming itself.

mod from_context;
mod payload;
mod validated;

pub use from_context::{BodyAlreadyRead, BodyExtractionError, FromContext, take_body};
pub use payload::Payload;
pub use validated::{ValidatableExtractor, Validated, ValidationError};
