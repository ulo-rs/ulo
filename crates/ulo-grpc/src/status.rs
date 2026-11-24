//! Kinds to canonical codes, `grpc-status-details-bin`, and the HTTP-status table for a response a
//! pre-dispatch entry answered (transports DESIGN §2.4, §6.2).

use std::collections::HashMap;
use std::time::Duration;

use bytes::Bytes;
use http::StatusCode;
use prost::Message as _;
use prost_types::value::Kind;
use tonic::{Code, Status};
use tonic_types::{ErrorDetails, FieldViolation, HelpLink, StatusExt};
use ulo::{AppHandle, BoxError, CancelReason, ExecutionRef};
use ulo_transport::{CallError, Detail, Details, ErrorKind, ExtractError};

use crate::extract::Undecodable;

/// The type URL a `Detail::Json` is packed under: a `google.protobuf.Value` holds any JSON value.
const VALUE_TYPE_URL: &str = "type.googleapis.com/google.protobuf.Value";

/// The canonical code for `kind`: `BadRequest` and `Unprocessable` INVALID_ARGUMENT, `Conflict`
/// ABORTED, `Timeout` DEADLINE_EXCEEDED, `TooManyRequests` RESOURCE_EXHAUSTED, `Cancelled`
/// CANCELLED, and so on.
pub fn code_for(kind: ErrorKind) -> Code {
    match kind {
        ErrorKind::BadRequest | ErrorKind::Unprocessable => Code::InvalidArgument,
        ErrorKind::Unauthorized => Code::Unauthenticated,
        ErrorKind::Forbidden => Code::PermissionDenied,
        ErrorKind::NotFound => Code::NotFound,
        ErrorKind::Conflict => Code::Aborted,
        ErrorKind::TooManyRequests => Code::ResourceExhausted,
        ErrorKind::Timeout => Code::DeadlineExceeded,
        ErrorKind::Unavailable => Code::Unavailable,
        ErrorKind::Unimplemented => Code::Unimplemented,
        ErrorKind::Cancelled => Code::Cancelled,
        _ => Code::Internal,
    }
}

/// The code for a non-200 HTTP response a pre-dispatch entry answered, by the gRPC specification's
/// table: 400 INTERNAL, 401 UNAUTHENTICATED, 403 PERMISSION_DENIED, 404 UNIMPLEMENTED, 429, 502,
/// 503 and 504 UNAVAILABLE, any other UNKNOWN.
pub fn code_for_http(status: StatusCode) -> Code {
    match status.as_u16() {
        400 => Code::Internal,
        401 => Code::Unauthenticated,
        403 => Code::PermissionDenied,
        404 => Code::Unimplemented,
        429 | 502 | 503 | 504 => Code::Unavailable,
        _ => Code::Unknown,
    }
}

/// Sends a [`CallError`] over gRPC with a canonical code of its own in place of the one its kind
/// maps to. A `Conflict` maps to `ABORTED`, and Google's API guidance pairs HTTP 409 with
/// `ALREADY_EXISTS` too: an error naming a resource that exists writes
/// `.with_grpc_code(Code::AlreadyExists)`. The kind still decides every other transport's
/// rendering, the HTTP status, the RPC and WebSocket envelopes' `kind` and a WebSocket connect
/// refusal's close code, so the error keeps one meaning across transports. `Code::Ok` is not an
/// error's code and is ignored, logged at `warn` when the error is rendered: an error sent as `OK`
/// would read as success.
///
/// A handler can also return a `tonic::Status` as its error. The error handlers see it as it was
/// returned, and one none of them claims or replaces is sent unchanged, its code, message and
/// details as built, with no kind consulted. An `OK` status is no error's and is sent as
/// [`from_status`] reads it, INTERNAL.
///
/// ```
/// use ulo_grpc::{CallError, ErrorKind, WithGrpcCode, to_status};
/// use tonic::Code;
///
/// let error = CallError::new(ErrorKind::Conflict, "the user exists").with_grpc_code(Code::AlreadyExists);
/// assert_eq!(error.kind(), ErrorKind::Conflict);
/// assert_eq!(error.grpc_code(), Some(Code::AlreadyExists));
/// assert_eq!(to_status(&error).code(), Code::AlreadyExists);
/// ```
///
/// Sealed: [`CallError`] is the one implementor, the store the renderer reads, so the trait can
/// gain a method without breaking a type outside this crate.
///
/// ```compile_fail,E0277
/// use tonic::Code;
/// use ulo_grpc::WithGrpcCode;
///
/// struct Mine;
///
/// impl WithGrpcCode for Mine {
///     fn with_grpc_code(self, _code: Code) -> Self {
///         self
///     }
///
///     fn grpc_code(&self) -> Option<Code> {
///         None
///     }
/// }
/// ```
pub trait WithGrpcCode: sealed::Sealed {
    fn with_grpc_code(self, code: Code) -> Self;

    /// The code [`with_grpc_code`](Self::with_grpc_code) set, as it was set, so an error handler
    /// rebuilding the error keeps it; for an error [`from_status`] built, the status's code.
    /// `None` when none was set, and the kind's code ([`code_for`]) applies. `Code::Ok` is answered
    /// here too, though the renderer ignores it and sends the kind's.
    fn grpc_code(&self) -> Option<Code>;
}

mod sealed {
    pub trait Sealed {}

    impl Sealed for ulo_transport::CallError {}
}

impl WithGrpcCode for CallError {
    fn with_grpc_code(self, code: Code) -> Self {
        self.__with_grpc_code(i32::from(code))
    }

    fn grpc_code(&self) -> Option<Code> {
        self.__grpc_code().and_then(code_of)
    }
}

/// The code numbered `written`, or `None` for a number no code has, which only the doc-hidden
/// store can write: `Code::from_i32` reads such a number as `Unknown`, which was never set.
fn code_of(written: i32) -> Option<Code> {
    let code = Code::from_i32(written);
    (i32::from(code) == written).then_some(code)
}

/// The code [`WithGrpcCode`] set on `err`, when it is one of the sixteen error codes. `OK`, or a
/// number no code has, is logged at `warn` and read as unset.
fn own_code(err: &CallError) -> Option<Code> {
    let written = err.__grpc_code()?;
    if (1..=16).contains(&written) {
        return Some(Code::from_i32(written));
    }
    tracing::warn!(code = written, kind = %err.kind(), "an error's gRPC code is not an error's; it is sent with its kind's");
    None
}

/// `err` as a status: its kind's code, or the one [`WithGrpcCode`] set, its public message, and
/// its details packed into a `google.rpc.Status` in `grpc-status-details-bin` through
/// tonic-types, a `Detail::Json` as a `google.protobuf.Value` packed in an `Any`. An error with no
/// details sends no `grpc-status-details-bin`.
///
/// Two extraction failures have a code of their own, as HTTP answers `TooLarge` 413: an
/// `ExtractError::TooLarge`, a message past the 4 MiB limit as it arrives or once decompressed, is
/// RESOURCE_EXHAUSTED, the code gRPC implementations give a message past their receive limit; a
/// request body that does not decode, breaking the message framing or holding a message prost
/// does not decode, is INTERNAL, which the compression specification requires for a compressed
/// flag on a call naming no compression and gRPC's status code table gives "error parsing request
/// proto". The error handlers read either by its kind, `TooLarge`'s and `Malformed`'s
/// `BadRequest`; the code is the wire's alone.
///
/// tonic-types' `ErrorDetails` holds one `BadRequest`, one `ErrorInfo`, one `RetryInfo` and one
/// `Help`: every `FieldViolations` detail's violations are written into the one `BadRequest`, every
/// `Help` detail's links into the one `Help`, and the first `ErrorInfo` and `RetryAfter` are kept.
pub fn to_status(err: &CallError) -> Status {
    let code = own_code(err).or_else(|| extraction_code(err)).unwrap_or_else(|| code_for(err.kind()));
    with_details(code, err.message(), err.details())
}

/// The code of an extraction failure the gRPC wire names apart from its kind.
fn extraction_code(err: &CallError) -> Option<Code> {
    match err.source_as::<ExtractError>()? {
        ExtractError::TooLarge { .. } => Some(Code::ResourceExhausted),
        ExtractError::Malformed { source, .. } if source.downcast_ref::<Undecodable>().is_some() => Some(Code::Internal),
        _ => None,
    }
}

/// `status` as a `CallError` that is sent as `status`, its code, message, details and metadata
/// as built: what a reply stream's `Err` item, whose error converts into a `CallError`, carries to
/// end the stream with a status of its own. The status's metadata is written in the trailers that
/// end the stream, beside the trailers the reply set, a name gRPC over HTTP/2 reserves dropped as
/// [`MetadataEntry`](crate::MetadataEntry) refuses it.
///
/// The error handlers see a `CallError` whose kind is the code's, `NOT_FOUND` a `NotFound` and a
/// code no kind names `Internal`, with `status` as its source, `source_as::<tonic::Status>()`,
/// and the status's code as its own, [`WithGrpcCode::grpc_code`], so the error reworded,
/// summarised or rebuilt from its read-backs is still sent with that code: a FAILED_PRECONDITION
/// stays FAILED_PRECONDITION where its kind, `BadRequest`, would send INVALID_ARGUMENT.
/// An `OK` status is not an error: it is read as a server bug, a `CallError` of kind `Internal`
/// whose message says an OK status was used as an error, sent as INTERNAL, with `status` still its
/// source and no code of its own.
/// A `CallError` that names a status as its cause with a kind, message, details or code of its
/// own renders by those: `CallError::new(ErrorKind::Unavailable, "the upstream failed")
/// .with_source(status)` is sent as UNAVAILABLE, so a status a handler received from another
/// server is not sent on as its reply.
///
/// ```
/// use futures_util::stream;
/// use tonic::Status;
/// use tonic::metadata::MetadataValue;
/// use ulo_grpc::CallError;
///
/// let mut status = Status::aborted("the feed was reset");
/// status.metadata_mut().insert_bin("x-cursor-bin", MetadataValue::from_bytes(&[0x01, 0x02]));
/// let items = stream::iter([Ok::<u32, CallError>(1), Err(ulo_grpc::from_status(status))]);
/// # let _ = items;
/// ```
pub fn from_status(status: Status) -> CallError {
    if status.code() == Code::Ok {
        return CallError::new(ErrorKind::Internal, OK_AS_ERROR).with_source(status);
    }
    CallError::new(kind_for(status.code()), status.message().to_owned()).with_grpc_code(status.code()).with_source(status)
}

/// The message of the `Internal` [`from_status`] answers for an `OK` status.
const OK_AS_ERROR: &str = "an OK status was used as an error";

/// The status `call` is sent as: its source, while `call` is as [`from_status`] built it, the kind
/// the code reads as, the status's message and its code, with no details or challenge of its own.
/// An `OK` status is never sent as an error's.
pub(crate) fn carried(call: &CallError) -> Option<&Status> {
    let status = call.source_as::<Status>()?;
    let as_built = status.code() != Code::Ok
        && call.kind() == kind_for(status.code())
        && call.message() == status.message()
        && call.details().is_empty()
        && call.grpc_code() == Some(status.code())
        && call.challenge().is_none();
    as_built.then_some(status)
}

/// The kind a canonical code is read as, the inverse of [`code_for`] where one exists:
/// ALREADY_EXISTS a `Conflict` beside ABORTED, and every code no kind names `Internal`.
fn kind_for(code: Code) -> ErrorKind {
    match code {
        Code::InvalidArgument | Code::OutOfRange | Code::FailedPrecondition => ErrorKind::BadRequest,
        Code::Unauthenticated => ErrorKind::Unauthorized,
        Code::PermissionDenied => ErrorKind::Forbidden,
        Code::NotFound => ErrorKind::NotFound,
        Code::Aborted | Code::AlreadyExists => ErrorKind::Conflict,
        Code::ResourceExhausted => ErrorKind::TooManyRequests,
        Code::DeadlineExceeded => ErrorKind::Timeout,
        Code::Unavailable => ErrorKind::Unavailable,
        Code::Unimplemented => ErrorKind::Unimplemented,
        Code::Cancelled => ErrorKind::Cancelled,
        _ => ErrorKind::Internal,
    }
}

/// The status a failed call is answered with, `err` being what the error handlers left: the
/// `tonic::Status` a handler or an error handler returned as it stands, an `OK` one as
/// [`from_status`] reads it, DEADLINE_EXCEEDED when the call's deadline cancelled it, and
/// otherwise the error as `CallError::from_boxed` recognises it.
pub(crate) fn render(err: BoxError, exec: &ExecutionRef, app: &AppHandle) -> Status {
    if exec.cancel_reason() == Some(CancelReason::Deadline) && !err.is::<Status>() {
        return deadline_exceeded();
    }
    render_as_is(err, app)
}

/// The status for `err` whatever the execution's cancel reason: what the error handlers of a
/// passed deadline return, which is the `Timeout` it offered or their reshaping of it. A
/// `tonic::Status` as it stands, an `OK` one as [`from_status`] reads it, a `CallError`
/// [`from_status`] built as that status, any other error as `CallError::from_boxed` recognises
/// it; the status's metadata screened by [`screened`].
///
/// A `CallError`'s message and details go through the app's redaction (`CallError::new`) before
/// anything else reads them, so one [`from_status`] built whose text held a registered secret is
/// sent as an error of its code with the text redacted, not as the status it was built from.
pub(crate) fn render_as_is(err: BoxError, app: &AppHandle) -> Status {
    rendered(err, |call| call.__into_sent(app))
}

/// [`render_as_is`], `sent` giving the `CallError` as it is sent.
fn rendered(err: BoxError, sent: impl FnOnce(CallError) -> CallError) -> Status {
    let status = match err.downcast::<Status>() {
        Ok(status) if status.code() != Code::Ok => *status,
        Ok(status) => to_status(&from_status(*status)),
        Err(err) => {
            let call = sent(CallError::from_boxed(err));
            match carried(&call) {
                Some(status) => status.clone(),
                None => to_status(&call),
            }
        }
    };
    screened(status)
}

/// `status` without the metadata names gRPC over HTTP/2 reserves, each dropped logged at `warn`:
/// a handler's `tonic::Status` sets no `grpc-` name beside the ones its code, message and details
/// write.
pub(crate) fn screened(mut status: Status) -> Status {
    crate::metadata::screened(status.metadata_mut());
    status
}

/// The `Timeout` a passed deadline offers the error handlers.
pub(crate) fn timed_out() -> CallError {
    CallError::new(ErrorKind::Timeout, "the call's deadline passed")
}

/// DEADLINE_EXCEEDED, for a passed deadline no error handler answered.
pub(crate) fn deadline_exceeded() -> Status {
    to_status(&timed_out())
}

fn with_details(code: Code, message: &str, details: &Details) -> Status {
    if details.is_empty() {
        return Status::new(code, message);
    }
    let mut violations: Vec<FieldViolation> = Vec::new();
    let mut links: Vec<HelpLink> = Vec::new();
    let mut info = None;
    let mut retry: Option<Duration> = None;
    let mut values = Vec::new();
    for detail in details {
        match detail {
            Detail::FieldViolations(list) => {
                violations.extend(list.iter().map(|violation| FieldViolation::new(&violation.field, &violation.description)));
            }
            Detail::ErrorInfo { reason, domain, metadata } => {
                if info.is_none() {
                    info = Some((reason, domain, metadata));
                }
            }
            Detail::RetryAfter(after) => {
                if retry.is_none() {
                    retry = Some(*after);
                }
            }
            Detail::Help(list) => links.extend(list.iter().map(|link| HelpLink::new(&link.description, &link.url))),
            Detail::Json(value) => values.push(value),
            _ => {}
        }
    }
    let mut rich = ErrorDetails::new();
    if !violations.is_empty() {
        rich.set_bad_request(violations);
    }
    if let Some((reason, domain, metadata)) = info {
        let metadata: HashMap<String, String> = metadata.iter().map(|(key, value)| (key.clone(), value.clone())).collect();
        rich.set_error_info(reason.clone(), domain.clone(), metadata);
    }
    if retry.is_some() {
        rich.set_retry_info(retry);
    }
    if !links.is_empty() {
        rich.set_help(links);
    }
    let status = Status::with_error_details(code, message, rich);
    if values.is_empty() {
        return status;
    }
    // tonic-types has no setter for an arbitrary value, so the `google.rpc.Status` it wrote is read
    // back and the values appended to its `details`.
    let mut packed = tonic_types::Status::decode(status.details())
        .unwrap_or_else(|_| tonic_types::Status { code: i32::from(code), message: message.to_owned(), details: Vec::new() });
    for value in values {
        packed.details.push(prost_types::Any { type_url: VALUE_TYPE_URL.to_owned(), value: protobuf_value(value).encode_to_vec() });
    }
    Status::with_details(code, message, Bytes::from(packed.encode_to_vec()))
}

/// `json` as a `google.protobuf.Value`. A number `serde_json` cannot read as an `f64`, which only
/// its `arbitrary_precision` feature produces, is carried as its text.
fn protobuf_value(json: &serde_json::Value) -> prost_types::Value {
    let kind = match json {
        serde_json::Value::Null => Kind::NullValue(prost_types::NullValue::NullValue as i32),
        serde_json::Value::Bool(value) => Kind::BoolValue(*value),
        serde_json::Value::Number(number) => match number.as_f64() {
            Some(value) => Kind::NumberValue(value),
            None => Kind::StringValue(number.to_string()),
        },
        serde_json::Value::String(text) => Kind::StringValue(text.clone()),
        serde_json::Value::Array(items) => Kind::ListValue(prost_types::ListValue { values: items.iter().map(protobuf_value).collect() }),
        serde_json::Value::Object(members) => Kind::StructValue(prost_types::Struct {
            fields: members.iter().map(|(name, value)| (name.clone(), protobuf_value(value))).collect(),
        }),
    };
    prost_types::Value { kind: Some(kind) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An `OK` status is a server bug as an error: INTERNAL, saying so, and the status kept.
    #[test]
    fn an_ok_status_from_status_builds_is_sent_as_internal() {
        let call = from_status(Status::ok("done"));
        assert_eq!((call.kind(), call.message()), (ErrorKind::Internal, OK_AS_ERROR));
        assert_eq!(call.source_as::<Status>().map(Status::code), Some(Code::Ok), "the status stays the source");
        let sent = rendered(Box::new(call), |call| call);
        assert_eq!((sent.code(), sent.message()), (Code::Internal, OK_AS_ERROR));
    }

    /// A `CallError` shaped as `from_status` would build one from an `OK` status, kind `Internal`
    /// and the status's message, is still not sent as that status.
    #[test]
    fn an_ok_status_is_never_carried_as_an_error_s_status() {
        let call = CallError::new(ErrorKind::Internal, "done").with_source(Status::ok("done"));
        assert!(carried(&call).is_none());
        assert_eq!(rendered(Box::new(call), |call| call).code(), Code::Internal);
    }

    /// An `OK` status returned bare as the error, by a handler or an error handler, is the same
    /// server bug.
    #[test]
    fn a_bare_ok_status_is_sent_as_internal() {
        let sent = rendered(Box::new(Status::ok("done")), |call| call);
        assert_eq!((sent.code(), sent.message()), (Code::Internal, OK_AS_ERROR));
        assert_eq!(rendered(Box::new(Status::not_found("gone")), |call| call).code(), Code::NotFound, "any other status as it stands");
    }

    /// A status `from_status` built is sent as the status itself, metadata included, while the
    /// error's code is the status's; a code of the error's own is sent in its place.
    #[test]
    fn a_status_is_carried_while_the_error_s_code_is_the_status_s() {
        let call = || {
            let mut status = Status::failed_precondition("stale");
            status.metadata_mut().insert("x-cursor", tonic::metadata::MetadataValue::from_static("7"));
            from_status(status)
        };
        assert_eq!(call().grpc_code(), Some(Code::FailedPrecondition));
        assert_eq!(from_status(Status::not_found("gone")).grpc_code(), Some(Code::NotFound), "a code its kind sends too");
        let sent = rendered(Box::new(call()), |call| call);
        assert_eq!((sent.code(), sent.metadata().get("x-cursor").and_then(|value| value.to_str().ok())), (Code::FailedPrecondition, Some("7")));
        let renamed = call().with_grpc_code(Code::OutOfRange);
        assert!(carried(&renamed).is_none(), "an error naming another code is not the status");
        assert_eq!(rendered(Box::new(renamed), |call| call).code(), Code::OutOfRange);
    }

    /// A number outside 1 to 16 reaches the store only past `WithGrpcCode`, and is read as unset.
    #[test]
    fn a_stored_code_that_is_not_an_error_s_is_sent_as_the_kind_s() {
        for written in [0, -1, 17] {
            let call = CallError::new(ErrorKind::Conflict, "exists").__with_grpc_code(written);
            assert_eq!(to_status(&call).code(), Code::Aborted, "{written}");
        }
        let named = CallError::new(ErrorKind::Conflict, "exists").with_grpc_code(Code::AlreadyExists);
        assert_eq!(to_status(&named).code(), Code::AlreadyExists);
    }

    /// Every code reads back as it was set, `OK` among them though the renderer ignores it; a
    /// number no code has reads back unset rather than as `Unknown`.
    #[test]
    fn grpc_code_answers_the_code_as_set() {
        assert_eq!(CallError::new(ErrorKind::Conflict, "exists").grpc_code(), None);
        for number in 0..=16 {
            let code = Code::from_i32(number);
            let call = CallError::new(ErrorKind::Conflict, "exists").with_grpc_code(code);
            assert_eq!(call.grpc_code(), Some(code), "{number}");
        }
        for written in [-1, 17] {
            let call = CallError::new(ErrorKind::Conflict, "exists").__with_grpc_code(written);
            assert_eq!(call.grpc_code(), None, "{written}");
        }
    }
}
