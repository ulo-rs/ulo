//! What an HTTP handler asks for.
//!
//! Two shapes, and which one an extractor is written in decides whether it touches the body:
//!
//! - Metadata — headers, path params, query params, extensions. Borrow the request parts from
//!   [`HttpContext::request`], which leaves the body untouched. [`Path`] and [`Query`] are written
//!   this way, and any number of them can run on one handler.
//!
//! - The body. Take it with [`take_body`](crate::extract::take_body), which yields it once and
//!   names the second asker. [`Json`], [`Bytes`], [`Body`], [`BodyStream`] and [`Multipart`] are
//!   written this way, and only one of them can run on a handler.
//!
//! [`HttpContext::request`]: crate::http::HttpContext::request

mod body;
mod body_stream;
mod bytes;
mod json;
mod multipart;
mod path;
mod query;

pub use body::Body;
pub use body_stream::BodyStream;
pub use bytes::Bytes;
pub use json::Json;
pub use multipart::{Field, Multipart, MultipartError};
pub use path::Path;
pub use query::Query;
