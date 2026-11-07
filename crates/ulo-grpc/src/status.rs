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
use ulo::{BoxError, CancelReason, ExecutionRef};
use ulo_transport::{CallError, Detail, Details, ErrorKind};

/// The type URL a `Detail::Json` is packed under: a `google.protobuf.Value` holds any JSON value.
const VALUE_TYPE_URL: &str = "type.googleapis.com/google.protobuf.Value";

/// The canonical code for `kind`: `BadRequest` and `Unprocessable` INVALID_ARGUMENT, `Conflict`
/// ABORTED, `Timeout` DEADLINE_EXCEEDED, `TooManyRequests` RESOURCE_EXHAUSTED, and so on.
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

/// `err` as a status: its kind's code, or the one `CallError::with_grpc_code` set, its public
/// message, and its details packed into a `google.rpc.Status` in `grpc-status-details-bin` through
/// tonic-types, a `Detail::Json` as a `google.protobuf.Value` packed in an `Any`. An error with no
/// details sends no `grpc-status-details-bin`.
///
/// tonic-types' `ErrorDetails` holds one `BadRequest`, one `ErrorInfo`, one `RetryInfo` and one
/// `Help`: every `FieldViolations` detail's violations are written into the one `BadRequest`, every
/// `Help` detail's links into the one `Help`, and the first `ErrorInfo` and `RetryAfter` are kept.
pub fn to_status(err: &CallError) -> Status {
    let code = err
        .grpc_code()
        .filter(|code| (1..=16).contains(code))
        .map(Code::from_i32)
        .unwrap_or_else(|| code_for(err.kind()));
    with_details(code, err.message(), err.details())
}

/// `status` as a `CallError` that is sent as `status`, its code, message, details and metadata
/// as built: what a reply stream's `Err` item, whose error converts into a `CallError`, carries to
/// end the stream with a status of its own. The status's metadata is written in the trailers that
/// end the stream, beside the trailers the reply set, a name gRPC over HTTP/2 reserves dropped as
/// [`MetadataEntry`](crate::MetadataEntry) refuses it.
///
/// The error handlers see a `CallError` whose kind is the code's, `NOT_FOUND` a `NotFound` and a
/// code no kind names `Internal`, with `status` as its source, `source_as::<tonic::Status>()`.
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
    CallError::new(kind_for(status.code()), status.message().to_owned()).with_source(status)
}

/// The status `call` is sent as: its source, while `call` is as [`from_status`] built it, the kind
/// the code reads as and the status's message, with no details, code or challenge of its own.
pub(crate) fn carried(call: &CallError) -> Option<&Status> {
    let status = call.source_as::<Status>()?;
    let as_built = call.kind() == kind_for(status.code())
        && call.message() == status.message()
        && call.details().is_empty()
        && call.grpc_code().is_none()
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
        _ => ErrorKind::Internal,
    }
}

/// The status a failed call is answered with, `err` being what the error handlers left: the
/// `tonic::Status` a handler or an error handler returned as it stands, DEADLINE_EXCEEDED when the
/// call's deadline cancelled it, and otherwise the error as `CallError::from_boxed` recognises it.
pub(crate) fn render(err: BoxError, exec: &ExecutionRef) -> Status {
    if exec.cancel_reason() == Some(CancelReason::Deadline) && !err.is::<Status>() {
        return deadline_exceeded();
    }
    render_as_is(err)
}

/// The status for `err` whatever the execution's cancel reason: what the error handlers of a
/// passed deadline return, which is the `Timeout` it offered or their reshaping of it. A
/// `tonic::Status` as it stands, a `CallError` [`from_status`] built as that status, any other error
/// as `CallError::from_boxed` recognises it; the status's metadata screened by [`screened`].
pub(crate) fn render_as_is(err: BoxError) -> Status {
    let status = match err.downcast::<Status>() {
        Ok(status) => *status,
        Err(err) => {
            let call = CallError::from_boxed(err);
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
