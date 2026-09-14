//! What runs around a handler: guards, interceptors and the error chain.
//!
//! Each is generic over the context, so one type serves one transport, all four, or any subset —
//! `impl Guard<HttpContext> for T` is the registration, and the framework reads which transports a
//! provider serves from the impls it carries.
//!
//! Each answers by returning. A guard answers `bool`, an interceptor answers the transport's
//! response type and short-circuits by not calling `next`, and an error handler answers
//! `Option<R>`, claiming the error or passing it along.

mod error_handler;
mod guard;
mod interceptor;

pub use error_handler::{ChainError, ErrorHandler};
pub use guard::Guard;
pub use interceptor::{Interceptor, InterceptorNext};
