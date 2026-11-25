//! One error model across transports (transports DESIGN §2.4): a domain error declares a
//! transport-neutral [`ErrorKind`] through [`Classify`], becomes a [`CallError`], and each
//! transport renders that canonically.

use std::any::Any;
use std::borrow::Cow;
use std::error::Error;
use std::fmt;

use ulo::{AppHandle, BoxError, Closed, FailureReason, GuardRejected, LookupError, PanicRecovered, Redacted, is_panic};

use crate::details::Details;
use crate::extract::ExtractError;

/// The transport-neutral kind of a failure, which each transport maps to its own status: HTTP
/// status codes, gRPC canonical codes, the WebSocket and RPC envelope's `kind` string.
#[non_exhaustive]
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ErrorKind {
    BadRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Unprocessable,
    TooManyRequests,
    /// The call ran out of time: gRPC DEADLINE_EXCEEDED and HTTP 504 Gateway Timeout, the HTTP
    /// mapping `google.rpc.Code` documents for DEADLINE_EXCEEDED (`code.proto`). grpc-gateway
    /// follows that file, and AIP-193 has an API's HTTP/1.1+JSON error carry the HTTP status
    /// corresponding to its code. HTTP's 408 Request Timeout is another case: the server gave up
    /// waiting for the client's request.
    Timeout,
    Unavailable,
    Unimplemented,
    Internal,
    /// The caller went away before the call completed: it reset the stream, closed the connection
    /// or cancelled the call while its request was read. gRPC's CANCELLED, which `statuscodes.md`
    /// describes as "typically by the caller"; HTTP's 499, the status nginx logs for a client that
    /// closed the request. A caller that has gone reads no reply, so the kind is for the error
    /// handlers, the logs and the counters: a departed caller counted apart from a bad request
    /// and from the server's own unavailability.
    Cancelled,
}

impl ErrorKind {
    /// The WebSocket and RPC envelope's `kind`: `"bad_request"`, `"not_found"`, ..
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::BadRequest => "bad_request",
            ErrorKind::Unauthorized => "unauthorized",
            ErrorKind::Forbidden => "forbidden",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Conflict => "conflict",
            ErrorKind::Unprocessable => "unprocessable",
            ErrorKind::TooManyRequests => "too_many_requests",
            ErrorKind::Timeout => "timeout",
            ErrorKind::Unavailable => "unavailable",
            ErrorKind::Unimplemented => "unimplemented",
            ErrorKind::Internal => "internal",
            ErrorKind::Cancelled => "cancelled",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What [`Classify`] requires of a type: `std::error::Error + Send + Sync + 'static`, under one
/// name. Implemented for every such type and for no other, so it names the bound and adds none:
/// `T: ErrorBounds` holds exactly where `T: Error + Send + Sync + 'static` does, and an impl
/// written by hand conflicts with the blanket one.
///
/// `#[derive(Classify)]` checks it, so a type without `Error` gets one error naming this trait,
/// with the advice to derive `Display` and `Error` through thiserror. The error speaks of the
/// derive wherever the bound fails, so a bound of a user's own is better written as the four.
///
/// ```
/// use ulo_transport::ErrorBounds;
///
/// fn holds<E: ErrorBounds>() {}
/// holds::<std::fmt::Error>();
/// ```
#[diagnostic::on_unimplemented(
    message = "`#[derive(Classify)]` needs `{Self}` to implement `std::error::Error` and be `Send + Sync + 'static`",
    label = "not an `Error + Send + Sync + 'static` type",
    note = "derive `Display` and `Error` with thiserror: `#[derive(Debug, thiserror::Error, Classify)]`, with an `#[error(\"..\")]` on the type or on each variant"
)]
pub trait ErrorBounds: Error + Send + Sync + 'static {}

impl<T: Error + Send + Sync + 'static> ErrorBounds for T {}

/// A domain error that knows its transport-neutral kind.
///
/// `#[derive(Classify)]` with `#[classify(not_found)]` on the type or on each variant writes the
/// impl beside thiserror's `#[error(..)]`. Implementing it is what makes `?` work inside a
/// function returning `Result<_, CallError>`: the blanket `impl<E: Classify> From<E> for
/// CallError` converts the error and keeps it as the source.
///
/// [`CallError`] never implements `Classify`, and must not: with that impl the blanket `From`
/// would collide with std's `impl<T> From<T> for T` (E0119). That is why `CallError` carries an
/// inherent [`kind`](CallError::kind) instead.
pub trait Classify: Error + Send + Sync + 'static {
    fn classify(&self) -> ErrorKind;

    /// What the caller may read. Defaults to the error's `Display`; override it for an error
    /// whose text names internals.
    fn public_message(&self) -> Cow<'_, str> {
        self.to_string().into()
    }

    fn details(&self) -> Details {
        Details::default()
    }
}

/// The one concrete error every transport renders: a kind, a public message, typed details, and
/// the domain error it came from.
///
/// The message is what the client reads, and `Display` writes it. An error
/// [`from_boxed`](Self::from_boxed) does not recognise is `Internal` with the message
/// `internal error`, its own text withheld. The error it was built from, recognised or not, is
/// reachable only as the source: [`source_as`](Self::source_as) by type, or
/// `std::error::Error::source`, which is what a reporting error handler reads; no accessor of the
/// message or of the details reaches it.
pub struct CallError {
    kind: ErrorKind,
    message: String,
    details: Details,
    /// The `WWW-Authenticate` challenge an `Unauthorized` carries, overriding the server's default.
    challenge: Option<String>,
    /// The gRPC status code this error is sent with in place of its kind's, which `ulo-grpc`'s
    /// `WithGrpcCode` writes from a typed code.
    grpc_code: Option<i32>,
    /// The HTTP status this error is sent with in place of its kind's, which `ulo-http`'s
    /// `WithHttpStatus` writes from a typed status.
    http_status: Option<u16>,
    /// The WebSocket close code a connect refusal built from this error closes with in place of
    /// its kind's, which `ulo-ws`'s `WithCloseCode` writes from a validated code.
    close_code: Option<u16>,
    source: Option<BoxError>,
}

const _: () = assert!(size_of::<CallError>() < 128, "clippy's `result_large_err` fires at 128 bytes");

impl CallError {
    /// An error of `kind` whose public message, what the caller reads, is `message`.
    ///
    /// Every transport writes the message, and every string of the [details](Self::with_details),
    /// through the app's redaction (`ulo::AppHandle::redact_text`) as it sends them: the value of
    /// a `Secret` registered with the app is sent as `[redacted]`, as is the userinfo of anything
    /// shaped like a URL, on HTTP, gRPC, WebSocket and RPC alike, whoever built the error. A secret
    /// under four characters is not matched and is sent as written, since it would replace
    /// ordinary words. A message holding no registered secret and no URL userinfo is sent byte
    /// for byte. The error itself keeps the text as given: [`message`](Self::message) and an error
    /// handler read it unredacted.
    ///
    /// ```
    /// use ulo_transport::{CallError, ErrorKind};
    ///
    /// let error = CallError::new(ErrorKind::NotFound, "no tenant `acme`");
    /// assert_eq!((error.kind(), error.message()), (ErrorKind::NotFound, "no tenant `acme`"));
    /// ```
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        CallError {
            kind,
            message: message.into(),
            details: Details::default(),
            challenge: None,
            grpc_code: None,
            http_status: None,
            close_code: None,
            source: None,
        }
    }

    /// `Unauthorized`, carrying its own `WWW-Authenticate` challenge (RFC 9110 requires one on a
    /// 401); without one the HTTP server's configured default applies, `Bearer` unless set.
    ///
    /// A guard picks between two refusals. `Ok(false)` says the caller is known and may not: 403 on
    /// HTTP, `PERMISSION_DENIED` on gRPC. Failing with this says the caller has not shown who they
    /// are: 401 with the challenge on HTTP, `UNAUTHENTICATED` on gRPC.
    ///
    /// ```
    /// use ulo::{BoxError, Guard, Transport};
    /// use ulo_transport::CallError;
    ///
    /// struct SignedIn;
    ///
    /// impl<T: Transport> Guard<T> for SignedIn {
    ///     async fn can_activate(&self, _cx: &T::Cx) -> Result<bool, BoxError> {
    ///         let credentials: Option<&str> = None; // read from the call's headers or metadata
    ///         match credentials {
    ///             None => Err(CallError::unauthorized("Bearer").into()),
    ///             Some(token) => Ok(token == "admin"),
    ///         }
    ///     }
    /// }
    /// ```
    pub fn unauthorized(challenge: impl Into<String>) -> Self {
        let mut error = CallError::new(ErrorKind::Unauthorized, "unauthorized");
        error.challenge = Some(challenge.into());
        error
    }

    /// The recogniser: walks `err` and maps the core's own errors to a kind.
    ///
    /// | Error | Kind |
    /// |---|---|
    /// | `CallError`, bare or boxed by the reply probe | its own |
    /// | `ExtractError` | its `Classify` kind: `BadRequest`, `Unprocessable`, `Cancelled` for a caller that went away; a `Dependency` as its lookup error maps |
    /// | `GuardRejected` | `Forbidden` |
    /// | `PanicRecovered`, and anything `ulo::is_panic` recognises | `Internal`, with a generic message |
    /// | `LookupError::Construct { reason: Errored(r) }` | `r` by this table, so a constructor's `CallError` "tenant not found" is 404 |
    /// | `Closed`, `LookupError::Closed` | `Unavailable` |
    /// | anything else | `Internal`, with the message withheld |
    ///
    /// A named constructor rather than `From<BoxError>`: a recogniser that walks an error should
    /// not run invisibly on every `?`. A passed deadline is not in the error: a transport tests
    /// `exec.cancel_reason()` first and renders `Timeout` for `CancelReason::Deadline`.
    ///
    /// This names no transport's own error type, so a boxed `tonic::Status` or `ConnectRefused` is
    /// `Internal` here and its wire code is lost. A transport's renderer recognises its own types
    /// before calling this; an error handler that rebuilds an error calls its own transport's helper,
    /// `ulo_grpc::call_error` or `ulo_ws::call_error`, which does the same and falls back to this.
    ///
    /// The original is kept as the source. A [`Redacted`] holding one of the errors above, as
    /// `err` itself or as a constructor's error inside `LookupError::Construct`, maps as its
    /// original would: redaction changes the text, not the kind. No other `source()` chain is
    /// walked, so an outside error wrapping a `CallError` is `Internal`.
    pub fn from_boxed(err: BoxError) -> Self {
        let err = match err.downcast::<CallError>() {
            Ok(call) => return *call,
            Err(err) => err,
        };
        let err = match err.downcast::<ExtractError>() {
            Ok(extract) => return CallError::from_extract(*extract),
            Err(err) => err,
        };
        let recognised = if is_panic(&err) { None } else { recognise(as_error(&err), 0) };
        let mut call = recognised.unwrap_or_else(internal);
        call.source = Some(err);
        call
    }

    pub(crate) fn from_extract(err: ExtractError) -> Self {
        CallError::from(err)
    }

    pub fn with_details(mut self, details: Details) -> Self {
        self.details = details;
        self
    }

    /// The same error with `message` as its public message, for an error handler that localises
    /// or rewords what the client reads. The kind, the details, the challenge, every transport's
    /// status override and the source are kept, so the reply differs only in its text and the
    /// original stays reachable through [`source_as`](Self::source_as). The new message is
    /// redacted as it is sent, as [`new`](Self::new) describes.
    ///
    /// ```
    /// use ulo_transport::{CallError, ErrorKind};
    ///
    /// let error = CallError::unauthorized("Bearer realm=\"api\"").with_message("connectez-vous");
    /// assert_eq!(error.kind(), ErrorKind::Unauthorized);
    /// assert_eq!(error.message(), "connectez-vous");
    /// assert_eq!(error.challenge(), Some("Bearer realm=\"api\""));
    /// ```
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// Keeps `source` as the error an error handler reaches through [`source_as`](Self::source_as).
    pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
        self.source = Some(source.into());
        self
    }

    pub fn kind(&self) -> ErrorKind {
        self.kind
    }

    /// The public message as given: what a transport writes as `detail` or `message`, after the
    /// app's redaction.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The message and the details as a transport sends them: each string through
    /// `app.redact_text`, borrowed back when nothing in it was replaced. The one place every
    /// transport's renderer reads the caller-facing text from.
    #[doc(hidden)]
    pub fn __sent<'e>(&'e self, app: &AppHandle) -> (Cow<'e, str>, Cow<'e, Details>) {
        let message = app.redact_text(&self.message);
        let details = self.details.redacted(&|text| app.redact_text(text)).map_or(Cow::Borrowed(&self.details), Cow::Owned);
        (message, details)
    }

    /// The same error with the message and the details [`__sent`](Self::__sent) gives, every
    /// other field kept, the source among them; `self` unchanged when nothing was replaced.
    #[doc(hidden)]
    pub fn __into_sent(self, app: &AppHandle) -> CallError {
        let replaced = match self.__sent(app) {
            (Cow::Borrowed(_), Cow::Borrowed(_)) => None,
            (message, details) => Some((message.into_owned(), details.into_owned())),
        };
        match replaced {
            None => self,
            Some((message, details)) => CallError { message, details, ..self },
        }
    }

    pub fn details(&self) -> &Details {
        &self.details
    }

    pub fn challenge(&self) -> Option<&str> {
        self.challenge.as_deref()
    }

    /// The store behind `ulo_grpc::WithGrpcCode`, which takes a typed code and writes its number
    /// here. gRPC's renderer reads it back through [`__grpc_code`](Self::__grpc_code), and no other
    /// transport reads it.
    #[doc(hidden)]
    pub fn __with_grpc_code(mut self, code: i32) -> Self {
        self.grpc_code = Some(code);
        self
    }

    #[doc(hidden)]
    pub fn __grpc_code(&self) -> Option<i32> {
        self.grpc_code
    }

    /// The store behind `ulo_http::WithHttpStatus`, which takes a typed status and writes its
    /// number here; this crate names no protocol's types. HTTP's renderer reads it back through
    /// [`__http_status`](Self::__http_status), and no other transport reads it.
    #[doc(hidden)]
    pub fn __with_http_status(mut self, status: u16) -> Self {
        self.http_status = Some(status);
        self
    }

    #[doc(hidden)]
    pub fn __http_status(&self) -> Option<u16> {
        self.http_status
    }

    /// The store behind `ulo_ws::WithCloseCode`, which takes a validated close code and writes
    /// its number here. The WebSocket transport reads it back through
    /// [`__close_code`](Self::__close_code) when it refuses a connection, and no other transport
    /// reads it.
    #[doc(hidden)]
    pub fn __with_close_code(mut self, code: u16) -> Self {
        self.close_code = Some(code);
        self
    }

    #[doc(hidden)]
    pub fn __close_code(&self) -> Option<u16> {
        self.close_code
    }

    /// The domain error this was built from, by type: `err.downcast_ref::<CallError>()?.source_as::<UserError>()`.
    pub fn source_as<E: Error + 'static>(&self) -> Option<&E> {
        self.source.as_deref().and_then(|source| source.downcast_ref::<E>())
    }

    /// A copy without the source: kind, message, details, challenge and the per-transport
    /// overrides. What a transport keeps before handing the error to `ulo::dispatch_late`, which
    /// takes it by value, so it can still render the original when an error handler answers `Ok`
    /// on the late path.
    pub fn summary(&self) -> CallError {
        CallError {
            kind: self.kind,
            message: self.message.clone(),
            details: self.details.clone(),
            challenge: self.challenge.clone(),
            grpc_code: self.grpc_code,
            http_status: self.http_status,
            close_code: self.close_code,
            source: None,
        }
    }
}

impl<E: Classify> From<E> for CallError {
    fn from(e: E) -> Self {
        let mut call = match (&e as &dyn Any).downcast_ref::<ExtractError>() {
            Some(ExtractError::Dependency { source, .. }) => from_lookup(source, 0),
            _ => classified(&e),
        };
        call.source = Some(Box::new(e));
        call
    }
}

pub(crate) const INTERNAL_MESSAGE: &str = "internal error";
const FORBIDDEN_MESSAGE: &str = "forbidden";

/// The message of the `Unavailable` every transport answers a call it refuses, or abandons at the
/// drain's end, once the server is shutting down. The one source of the phrase: a transport
/// writes it from here and a test compares against it.
pub const SHUTDOWN_MESSAGE: &str = "the server is shutting down";

/// Bounded, in case a chain of `Redacted` and constructor errors loops back on itself.
const MAX_DEPTH: usize = 32;

/// `Internal` with the message withheld: the original may name keys, types or internals.
fn internal() -> CallError {
    CallError::new(ErrorKind::Internal, INTERNAL_MESSAGE)
}

fn classified<E: Classify + ?Sized>(e: &E) -> CallError {
    CallError::new(e.classify(), e.public_message().into_owned()).with_details(e.details())
}

fn as_error(err: &BoxError) -> &(dyn Error + 'static) {
    &**err
}

/// A source-less `CallError` for one of the errors `from_boxed` recognises, or `None` for any
/// other type.
fn recognise<'e>(error: impl ByType<'e>, depth: usize) -> Option<CallError> {
    if depth >= MAX_DEPTH {
        return None;
    }
    if let Some(call) = error.get::<CallError>() {
        return Some(call.summary());
    }
    if let Some(extract) = error.get::<ExtractError>() {
        return Some(match extract {
            ExtractError::Dependency { source, .. } => from_lookup(source, depth + 1),
            _ => classified(extract),
        });
    }
    if error.get::<PanicRecovered>().is_some() {
        return Some(internal());
    }
    if error.get::<GuardRejected>().is_some() {
        return Some(CallError::new(ErrorKind::Forbidden, FORBIDDEN_MESSAGE));
    }
    if let Some(lookup) = error.get::<LookupError>() {
        return Some(from_lookup(lookup, depth + 1));
    }
    if error.get::<Closed>().is_some() {
        return Some(CallError::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE));
    }
    if let Some(redacted) = error.get::<Redacted>() {
        return recognise(redacted, depth + 1);
    }
    None
}

/// A lookup error's kind: a constructor's own error maps as `from_boxed` maps it, so an
/// execution-scoped constructor's "tenant not found" is a 404; a lookup from Destroying on is
/// `Unavailable`; every other lookup failure, a panic or timeout inside the build included, is
/// `Internal` with the message withheld.
pub(crate) fn from_lookup(lookup: &LookupError, depth: usize) -> CallError {
    match lookup {
        LookupError::Construct { reason, .. } => match &**reason {
            FailureReason::Errored(cause) => recognise(cause, depth + 1).unwrap_or_else(internal),
            _ => internal(),
        },
        LookupError::Closed { .. } => CallError::new(ErrorKind::Unavailable, SHUTDOWN_MESSAGE),
        _ => internal(),
    }
}

/// The original by its type, which `dyn Error` and `Redacted` both answer.
trait ByType<'e>: Copy {
    fn get<E: Error + 'static>(self) -> Option<&'e E>;
}

impl<'e> ByType<'e> for &'e (dyn Error + 'static) {
    fn get<E: Error + 'static>(self) -> Option<&'e E> {
        self.downcast_ref::<E>()
    }
}

impl<'e> ByType<'e> for &'e Redacted {
    fn get<E: Error + 'static>(self) -> Option<&'e E> {
        self.downcast_ref::<E>()
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl fmt::Debug for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CallError")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .field("details", &self.details)
            .finish_non_exhaustive()
    }
}

impl Error for CallError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|source| source as &(dyn Error + 'static))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::details::Detail;

    #[derive(Debug)]
    struct Original;

    impl fmt::Display for Original {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("the original text")
        }
    }

    impl Error for Original {}

    /// Destructured whole, so a field added to `CallError` fails this test's build until the test
    /// says whether `with_message` keeps it.
    #[test]
    fn with_message_replaces_the_message_and_keeps_every_other_field() {
        let details = Details::new().with(Detail::RetryAfter(Duration::from_secs(3)));
        let rebuilt = CallError::unauthorized("Basic realm=\"shop\"")
            .with_details(details.clone())
            .with_source(Original)
            .__with_grpc_code(7)
            .__with_http_status(412)
            .__with_close_code(4403)
            .with_message("reworded");

        let CallError { kind, message, details: kept, challenge, grpc_code, http_status, close_code, source } = rebuilt;
        assert_eq!(kind, ErrorKind::Unauthorized);
        assert_eq!(message, "reworded");
        assert_eq!(kept, details);
        assert_eq!(challenge.as_deref(), Some("Basic realm=\"shop\""));
        assert_eq!((grpc_code, http_status, close_code), (Some(7), Some(412), Some(4403)));
        assert!(source.is_some_and(|source| source.downcast_ref::<Original>().is_some()), "the source is the original");
    }
}
