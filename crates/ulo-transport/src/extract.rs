//! Handler parameters built from the call (transports DESIGN §2.2).

use std::borrow::Cow;
use std::collections::HashSet;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::sync::{LazyLock, Mutex, PoisonError};

use ulo::{Dependencies, ExecutionRef, FromContainer, KeyName, LookupError, LookupKind, Redacted, Transport};

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

    /// Reads the parameter from the call. A failure is an [`ExtractError`]: every variant but
    /// `Dependency` classifies as `BadRequest`, `Unprocessable`, `Internal` or `Unavailable`, and
    /// none names `Unauthorized`, `Forbidden` or `NotFound`. `Dependency` takes the kind of the
    /// lookup error it holds, so a parameter that must fail with one of those reads a
    /// `FromContainer` type whose constructor returns a `CallError` of that kind: bare as the
    /// handler's parameter, or read inside `from_call` through the call's execution and declared
    /// in `dependencies`, its lookup error returned as `ExtractError::Dependency`.
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

    // `+ Send` in the signature rather than an `async fn`, for the reason `Param::extract` gives
    // (transports DESIGN decision 107): a caller naming `Option<Json<Name<'static>>>` would
    // otherwise read through to `P::from_call`'s future with its lifetimes generic.
    #[allow(clippy::manual_async_fn)]
    fn from_call(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            match P::from_call(cx).await {
                Ok(value) => Ok(Some(value)),
                Err(ExtractError::Missing { .. } | ExtractError::HostMissing { .. }) => Ok(None),
                Err(other) => Err(other),
            }
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

    // `+ Send` in the signature, as for `Option<P>` above.
    #[allow(clippy::manual_async_fn)]
    fn from_call(cx: &T::Cx) -> impl Future<Output = Result<Self, ExtractError>> + Send {
        async move {
            let exec: &ExecutionRef = cx.as_ref();
            let resolver = exec.resolver();
            match S::from_container(&resolver).await {
                Ok(value) => Ok(Injected(value)),
                Err(source) => {
                    let param = std::any::type_name::<S>();
                    if let LookupError::NotFound { key, kind: LookupKind::Extension } = &source {
                        unwritten_extension(exec, param, *key);
                    }
                    Err(ExtractError::Dependency { param, source })
                }
            }
        }
    }
}

/// The handlers and parameter types an unwritten extension has been logged for at `warn`. An entry
/// is made of names the compiled code holds, the handler's transport, controller and method and
/// the parameter's type, so the set is bounded by the application's handlers and grows with no
/// request data.
type Seen = HashSet<(&'static str, Option<KeyName>, &'static str, &'static str)>;
static UNWRITTEN_SEEN: LazyLock<Mutex<Seen>> = LazyLock::new(|| Mutex::new(HashSet::new()));

/// Logs a handler parameter whose extension nothing wrote: at `warn` the first time for each
/// handler and parameter type in the process, at `debug` after. The call answers 500, a missing
/// `Ext<T>` being a fault in the server rather than in the request. `extension` names the type
/// read, which differs from `param` when the read is a dependency's, an execution-scoped
/// `Dep<CurrentUser>` built from an `Ext<Session>` among them.
fn unwritten_extension(exec: &ExecutionRef, param: &'static str, extension: KeyName) {
    let info = exec.handler();
    let entry = match info {
        Some(info) => (info.transport(), Some(info.controller()), info.name(), param),
        None => ("", None, "", param),
    };
    let first = UNWRITTEN_SEEN.lock().unwrap_or_else(PoisonError::into_inner).insert(entry);
    let route = info.and_then(|info| info.route()).unwrap_or("");
    let handler = info.map(|info| format!("{}::{}", info.controller(), info.name())).unwrap_or_default();
    if first {
        tracing::warn!(
            param,
            extension = %format_args!("{extension:#}"),
            handler,
            route,
            "nothing wrote the extension this handler parameter reads earlier in the execution; answered 500, and later misses of it on this handler are logged at `debug`"
        );
    } else {
        tracing::debug!(param, extension = %format_args!("{extension:#}"), handler, route, "nothing wrote the extension this handler parameter reads earlier in the execution; answered 500");
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
    /// wrong type, a form lacking a field. A body that parses as JSON and is refused for what it
    /// holds is this whatever category the decoder files it under: a number past `f64`, nesting
    /// past 128. `source`, redacted as `Malformed`'s is, names what did not fit.
    Mismatched { param: &'static str, source: Redacted },
    UnsupportedMediaType { param: &'static str, expected: &'static str },
    /// Larger than `limit` bytes, the transport's bound on one body or one item: an HTTP body, a
    /// gRPC message, an RPC streamed request's item over `max_queued_bytes` on its own.
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
    /// A streamed body's items arrived faster than its reader took them, and the next one would
    /// have taken the bytes waiting unread past `limit`, the transport's byte bound. It ends the
    /// stream and classifies as `Overflowed` does. An item past the bound on its own is
    /// `TooLarge`, which no read makes room for.
    OverflowedBytes { param: &'static str, limit: u64 },
    /// A streamed body still being read when its call ended, before the caller's last item: a
    /// handler answered and left the body to a task of its own. The stream yields this as its
    /// last item, so that task does not take the part it read for the whole. It classifies as
    /// `Unavailable`; the call has been answered already.
    CallEnded { param: &'static str },
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
            | ExtractError::Overflowed { param, .. }
            | ExtractError::OverflowedBytes { param, .. }
            | ExtractError::CallEnded { param } => param,
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
            ExtractError::OverflowedBytes { param, limit } => {
                write!(f, "`{param}`'s next item would have taken the bytes waiting to be read past {limit}, and it ended the call")
            }
            ExtractError::CallEnded { param } => write!(f, "`{param}` was cut short: its call ended before the caller's last item"),
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
/// deployment's or the server's own, and the text names a type. `Overflowed` and `OverflowedBytes`
/// are `Unavailable`: the request was well formed and the server did not keep up with it, and
/// `CallEnded` is too: the server ended the call before the request did.
impl Classify for ExtractError {
    fn classify(&self) -> ErrorKind {
        match self {
            ExtractError::Invalid { .. } | ExtractError::Mismatched { .. } => ErrorKind::Unprocessable,
            ExtractError::Dependency { source, .. } => from_lookup(source, 0).kind(),
            ExtractError::HostMissing { .. } | ExtractError::BodyTaken { .. } => ErrorKind::Internal,
            ExtractError::Overflowed { .. } | ExtractError::OverflowedBytes { .. } | ExtractError::CallEnded { .. } => ErrorKind::Unavailable,
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
