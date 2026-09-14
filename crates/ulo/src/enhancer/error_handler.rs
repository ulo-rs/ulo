use crate::async_trait;
use crate::context::ExecutionContext;
use std::error::Error;

/// Convenience alias for the borrowed error reference passed to handlers.
///
/// The chain owns the boxed error and lends it to each handler in turn —
/// handlers borrow, downcast, and either claim (`Some`) or fall through
/// (`None`). Borrowing avoids forcing user errors to be `Clone` so each
/// chain iteration can re-box them.
pub type ChainError<'a> = &'a (dyn Error + Send + Sync + 'static);

/// Customize how errors are turned into responses.
///
/// Handlers are tried in order (method > controller > global) until one returns `Some`. Return
/// `None` to pass the error to the next one.
///
/// When every handler passes, the transport renders the error itself: the canonical envelope,
/// carrying the status, code or frame its [`ErrorKind`](crate::errors::ErrorKind) maps to. That
/// rendering is not an `ErrorHandler` and cannot be replaced by installing one — it reads
/// `kind()`, `message()` and `details()` off [`Error`](crate::errors::Error), and this trait is
/// handed the `std::error::Error` supertrait, which those do not reach.
#[async_trait]
pub trait ErrorHandler<C: ?Sized + ExecutionContext, R>: Send + Sync {
    async fn handle_error(&self, error: ChainError<'_>, ctx: &C) -> Option<R>;
}
