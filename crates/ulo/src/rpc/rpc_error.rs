//! RPC handler error type.
//!
//! `RpcError` is the error carried across the RPC dispatcher and adapter
//! boundary. Handlers may return any type implementing
//! [`ulo::Error`](crate::errors::Error) from their function body — the
//! [`From<E: Error>`] blanket lifts it into [`RpcError::AppError`] at
//! the macro boundary, and [`RpcError::to_data`] renders the canonical
//! envelope.
//!
//! `RpcError` does not implement [`ulo::Error`](crate::errors::Error) itself; the `From` blanket
//! requires source and target to be distinct types.

use std::fmt;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::errors::Error;
use crate::rpc::RpcData;

/// Variants the RPC dispatcher returns.
///
/// [`PatternNotFound`](Self::PatternNotFound), [`Forbidden`](Self::Forbidden),
/// and [`Internal`](Self::Internal) are emitted by the framework and reach
/// the adapter as wire-Err frames (`{"err":{"status":..., "message":...}}`).
/// [`AppError`](Self::AppError) carries a user-domain error and reaches the
/// adapter as a wire-Ok frame carrying the canonical envelope
/// (`{"response":{"status":"error","kind":..., "message":...}}`).
#[derive(Debug, Clone)]
pub enum RpcError {
    /// No registered handler matched the inbound pattern.
    PatternNotFound(String),

    /// A guard rejected the message before the handler ran.
    Forbidden(String),

    /// Generic server-side failure.
    Internal(String),

    /// Carries a user-domain error implementing [`ulo::Error`](crate::errors::Error). Constructed
    /// by the [`From<E: Error>`] blanket; handlers don't build this
    /// variant by hand.
    AppError(Arc<dyn Error + Send + Sync>),
}

impl RpcError {
    /// What this failure is, in the one vocabulary a wire payload names it by.
    ///
    /// The same string the envelope's `kind` carries, so a caller reading the frame and a caller
    /// reading the payload learn the failure by the same name.
    pub fn kind(&self) -> crate::errors::ErrorKind {
        match self {
            Self::AppError(e) => e.kind(),
            Self::PatternNotFound(_) => crate::errors::ErrorKind::NotFound,
            Self::Forbidden(_) => crate::errors::ErrorKind::Forbidden,
            Self::Internal(_) => crate::errors::ErrorKind::Internal,
        }
    }

    /// Render as an [`RpcData`] payload using the canonical envelope:
    /// `{"status":"error","kind":"...","message":...}`. For
    /// [`AppError`](Self::AppError), reads `kind` / `message` / `details`
    /// from the wrapped error; for the framework variants, uses a fixed
    /// `kind` per variant.
    ///
    /// What it returns is an answer, not a failure. A handler answers with the
    /// envelope by returning `Ok(err.to_data())` — `#[patterns]` takes the
    /// value inside a `Result`, and a bare `-> RpcData` does not compile. The
    /// error chain is never offered the error, and a `#[catch]` handler
    /// registered for it does not run. A handler that means to fail returns
    /// `Err`.
    ///
    /// The frame is the one an unclaimed failure from a controller produces, so
    /// nothing on the wire says which path wrote it. A dispatch failure is not:
    /// [`PatternNotFound`](Self::PatternNotFound) and
    /// [`Forbidden`](Self::Forbidden) leaving the dispatcher travel the
    /// `{"err":…}` lane.
    ///
    /// An error handler reaches this by catching [`RpcError`] itself. For
    /// [`AppError`](Self::AppError) the chain is handed the unwrapped domain
    /// error, which carries no renderer.
    pub fn to_data(&self) -> RpcData {
        match self {
            Self::AppError(e) => render_error(e.as_ref()),
            Self::PatternNotFound(m) => RpcData::json(json!({
                "status": "error",
                "kind": "NotFound",
                "message": m,
            })),
            Self::Forbidden(m) => RpcData::json(json!({
                "status": "error",
                "kind": "Forbidden",
                "message": m,
            })),
            Self::Internal(m) => RpcData::json(json!({
                "status": "error",
                "kind": "Internal",
                "message": m,
            })),
        }
    }
}

/// Render an arbitrary [`ulo::Error`] as the canonical RPC envelope.
/// Merges `details()` into the payload when present.
pub(crate) fn render_error(err: &dyn Error) -> RpcData {
    let mut payload = json!({
        "status": "error",
        "kind": err.kind().name(),
        "message": err.message(),
    });
    if let Some(details) = err.details()
        && let Value::Object(map) = &mut payload
    {
        map.insert("details".to_string(), details);
    }
    RpcData::json(payload)
}

impl fmt::Display for RpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PatternNotFound(m) => write!(f, "Pattern not found: {m}"),
            Self::Forbidden(m) => write!(f, "Guard rejected message: {m}"),
            Self::Internal(m) => write!(f, "Internal error: {m}"),
            Self::AppError(e) => write!(f, "{}: {}", e.kind().name(), e.message()),
        }
    }
}

impl std::error::Error for RpcError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AppError(e) => Some(e.as_ref()),
            _ => None,
        }
    }
}

/// Lift any [`ulo::Error`](crate::Error) into [`RpcError::AppError`]. Handlers returning
/// `Result<T, MyDomainError>` use this via `?` and via the macro's auto-
/// conversion at the dispatcher boundary.
impl<E: Error> From<E> for RpcError {
    fn from(e: E) -> Self {
        Self::AppError(Arc::new(e))
    }
}
