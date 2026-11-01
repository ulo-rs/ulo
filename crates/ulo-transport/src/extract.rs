//! Handler parameters built from the call (transports DESIGN §2.2).

use std::borrow::Cow;
use std::error::Error;
use std::fmt;
use std::future::Future;

use ulo::{Dependencies, ExecutionRef, FromContainer, LookupError, Redacted, Transport};

use crate::details::{Detail, Details, FieldViolation};
use crate::error::{Classify, ErrorKind, INTERNAL_MESSAGE, from_lookup};

/// A handler parameter built from the call.
///
/// The pair `FromContainer` / `FromCall` names where a parameter comes from, the container or the
/// call, and both appear in diagnostics. A transport implements `FromCall` for its own
/// extractors; a `FromContainer` type (`Dep`, `Many`, `Ext`, `ModuleRef`, `ExecutionRef`, a user
/// type) is accepted bare on every transport, the handler attribute reading it through the
/// container.
#[diagnostic::on_unimplemented(
    message = "`{Self}` cannot be built from a `{T}` call",
    note = "a transport implements `FromCall` for its own extractors; `FromContainer` types (`Dep`, `Ext`, ...) are accepted on every transport"
)]
pub trait FromCall<T: Transport>: Sized + Send + 'static {
    /// Whether this parameter consumes the body or payload. At most one may: a handler's
    /// attribute asserts it for every pair of parameters at compile time, naming both.
    const CONSUMES_BODY: bool = false;

    /// What it reads from the container, if anything, for the wiring pass.
    fn dependencies(_d: &mut Dependencies) {}

    fn from_call(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send;
}

/// `None` exactly where `P` fails with [`ExtractError::Missing`] or
/// [`ExtractError::HostMissing`]: a header, query field or body the call does not carry, or a
/// value its host did not supply. Every other failure propagates, [`ExtractError::BodyTaken`]
/// among them: a body the server took before the extractor ran is not one the call lacks. Forwards
/// `CONSUMES_BODY` and the dependencies.
impl<T: Transport, P: FromCall<T>> FromCall<T> for Option<P> {
    const CONSUMES_BODY: bool = P::CONSUMES_BODY;

    fn dependencies(d: &mut Dependencies) {
        P::dependencies(d);
    }

    async fn from_call(cx: &T::Cx) -> Result<Self, ExtractError> {
        match P::from_call(cx).await {
            Ok(value) => Ok(Some(value)),
            Err(ExtractError::Missing { .. } | ExtractError::HostMissing { .. }) => Ok(None),
            Err(other) => Err(other),
        }
    }
}

/// A `FromContainer` type as a `FromCall` parameter, read through the call's execution with the
/// controller's module visibility. A handler attribute accepts the type bare and reads it this
/// way; the wrapper is for code generic over `FromCall`.
pub struct Injected<S>(pub S);

impl<T, S> FromCall<T> for Injected<S>
where
    T: Transport,
    T::Cx: AsRef<ExecutionRef>,
    S: FromContainer,
{
    fn dependencies(d: &mut Dependencies) {
        d.add::<S>();
    }

    async fn from_call(cx: &T::Cx) -> Result<Self, ExtractError> {
        let exec: &ExecutionRef = cx.as_ref();
        let resolver = exec.resolver();
        match S::from_container(&resolver).await {
            Ok(value) => Ok(Injected(value)),
            Err(source) => Err(ExtractError::Dependency { param: std::any::type_name::<S>(), source }),
        }
    }
}

/// An extraction failure, one shape on every transport, each variant carrying the name of what
/// failed so one pattern matches it: the path segment, query field, header, `"body"`, or the
/// container type.
///
/// It reaches the error handlers as a `CallError` of its kind, holding the `ExtractError` as its
/// source; an error handler reshapes it with
/// `err.downcast_ref::<CallError>()?.source_as::<ExtractError>()`. Each transport documents its
/// rendering; HTTP answers 413 for `TooLarge` and 415 for `UnsupportedMediaType`.
#[non_exhaustive]
#[derive(Debug)]
pub enum ExtractError {
    /// A required header, query field or path segment.
    Missing { param: &'static str },
    /// Could not decode: the syntax is wrong, or the input ended early. `source` is built through
    /// `AppHandle::redact`, so a decoder's error echoing a credential is redacted like any outside
    /// error.
    Malformed { param: &'static str, source: Redacted },
    /// Decoded, and does not fit the type: a JSON body missing a field or holding a value of the
    /// wrong type, a form lacking a field. `source`, redacted as `Malformed`'s is, names what
    /// did not fit.
    Mismatched { param: &'static str, source: Redacted },
    UnsupportedMediaType { param: &'static str, expected: &'static str },
    TooLarge { param: &'static str, limit: u64 },
    /// Validation rules failed.
    Invalid { param: &'static str, violations: Vec<FieldViolation> },
    /// A `FromContainer` parameter's read failed: `param` is the type's name, `source` the
    /// lookup error, which classifies as `CallError::from_boxed` maps it, so an execution-scoped
    /// constructor's "tenant not found" is a 404 here too.
    Dependency { param: &'static str, source: LookupError },
    /// A value the server hosting the application was to supply, absent: a deployment fault
    /// rather than a bad request, so it classifies as `Internal` with its message withheld.
    /// `type_name` is the value's type.
    HostMissing { param: &'static str, type_name: &'static str },
    /// The body was taken before the extractor ran, by a guard or an interceptor calling the
    /// transport's `take_body`: a fault in the server, not in the request, so it classifies as
    /// `Internal` with its message withheld, and `Option<P>` does not read it as an absent body.
    /// `extractor` is the extractor's type.
    BodyTaken { param: &'static str, extractor: &'static str },
    /// A streamed body's items arrived faster than its reader took them: `limit` waited unread,
    /// the transport's bound, and the next one ended the call. The stream yields this as its last
    /// item, the items left unread dropped, so a reader does not take a request cut short for a
    /// complete one. It classifies as `Unavailable`. RPC's `Inbound<T>` yields it, and the RPC
    /// server answers the call `unavailable` with the reason `overflow` whatever the handler
    /// returns.
    Overflowed { param: &'static str, limit: u64 },
}

impl ExtractError {
    pub fn param(&self) -> &'static str {
        match self {
            ExtractError::Missing { param }
            | ExtractError::Malformed { param, .. }
            | ExtractError::Mismatched { param, .. }
            | ExtractError::UnsupportedMediaType { param, .. }
            | ExtractError::TooLarge { param, .. }
            | ExtractError::Invalid { param, .. }
            | ExtractError::Dependency { param, .. }
            | ExtractError::HostMissing { param, .. }
            | ExtractError::BodyTaken { param, .. }
            | ExtractError::Overflowed { param, .. } => param,
        }
    }
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExtractError::Missing { param } => write!(f, "`{param}` is missing"),
            ExtractError::Malformed { param, source } => write!(f, "`{param}` could not be decoded: {source}"),
            ExtractError::Mismatched { param, source } => write!(f, "`{param}` does not fit the expected type: {source}"),
            ExtractError::UnsupportedMediaType { param, expected } => write!(f, "`{param}` must be `{expected}`"),
            ExtractError::TooLarge { param, limit } => write!(f, "`{param}` is larger than {limit} bytes"),
            ExtractError::Invalid { param, violations } => {
                let count = violations.len();
                let noun = if count == 1 { "rule" } else { "rules" };
                write!(f, "`{param}` failed {count} validation {noun}")
            }
            ExtractError::Dependency { source, .. } => fmt::Display::fmt(source, f),
            ExtractError::HostMissing { param, type_name } => {
                write!(f, "`{param}`: the host supplied no `{type_name}` with the request")
            }
            ExtractError::BodyTaken { param, extractor } => write!(f, "`{param}` was taken before `{extractor}` read it"),
            ExtractError::Overflowed { param, limit } => {
                write!(f, "`{param}` had {limit} items waiting to be read, and the next one ended the call")
            }
        }
    }
}

impl Error for ExtractError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ExtractError::Dependency { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// `BadRequest`; `Unprocessable` for `Invalid`, with its violations as a `FieldViolations` detail,
/// and for `Mismatched`, content that decoded and does not fit, which RFC 9110 §15.5.21 gives 422
/// on HTTP; a `Dependency` takes the kind, message and details its lookup error maps to, so a
/// lookup failure's text, which names keys, is withheld as `CallError::from_boxed` withholds it.
/// `HostMissing` and `BodyTaken` are `Internal` with the message withheld: the fault is the
/// deployment's or the server's own, and the text names a type. `Overflowed` is `Unavailable`: the
/// request was well formed and the server did not keep up with it.
impl Classify for ExtractError {
    fn classify(&self) -> ErrorKind {
        match self {
            ExtractError::Invalid { .. } | ExtractError::Mismatched { .. } => ErrorKind::Unprocessable,
            ExtractError::Dependency { source, .. } => from_lookup(source, 0).kind(),
            ExtractError::HostMissing { .. } | ExtractError::BodyTaken { .. } => ErrorKind::Internal,
            ExtractError::Overflowed { .. } => ErrorKind::Unavailable,
            _ => ErrorKind::BadRequest,
        }
    }

    fn public_message(&self) -> Cow<'_, str> {
        match self {
            ExtractError::Dependency { source, .. } => Cow::Owned(from_lookup(source, 0).message().to_owned()),
            ExtractError::HostMissing { .. } | ExtractError::BodyTaken { .. } => Cow::Borrowed(INTERNAL_MESSAGE),
            _ => Cow::Owned(self.to_string()),
        }
    }

    fn details(&self) -> Details {
        match self {
            ExtractError::Invalid { violations, .. } => Details::new().with(Detail::FieldViolations(violations.clone())),
            ExtractError::Dependency { source, .. } => from_lookup(source, 0).details().clone(),
            _ => Details::new(),
        }
    }
}
