//! Kinds to canonical codes, `grpc-status-details-bin`, and the HTTP-status table for a response a
//! pre-dispatch entry answered (transports DESIGN §2.4, §6.2).

use std::borrow::Cow;
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
/// returned, and one none of them claims or replaces is sent with its code and metadata as built
/// and no kind consulted, its message and the text in its details passed through the app's
/// redaction. An `OK` status is no error's and is sent as [`from_status`] reads it, INTERNAL.
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

/// `status` as a `CallError` that is sent as `status`, its code, details and metadata as built
/// and its message and the text in its details passed through the app's redaction, as a bare
/// status's are: what a reply stream's `Err` item, whose error converts into a `CallError`,
/// carries to end the stream with a status of its own. A value that must reach the caller verbatim
/// goes in the metadata, which is not redacted, never in the message. The status's metadata is written in the trailers that
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

/// The `CallError` for an error an error handler holds as a `BoxError`: a `tonic::Status` as
/// [`from_status`] reads it, so its code, message, details and metadata are kept, and anything else
/// as `CallError::from_boxed` recognises it. An error handler that rebuilds an error calls this
/// rather than `from_boxed`, which names no tonic type and answers a boxed status `Internal`.
///
/// ```
/// use tonic::{Code, Status};
/// use ulo_grpc::WithGrpcCode;
///
/// let call = ulo_grpc::call_error(Box::new(Status::failed_precondition("stale")));
/// assert_eq!(call.grpc_code(), Some(Code::FailedPrecondition));
/// ```
pub fn call_error(err: BoxError) -> CallError {
    match err.downcast::<Status>() {
        Ok(status) => from_status(*status),
        Err(err) => CallError::from_boxed(err),
    }
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
/// `tonic::Status` a handler or an error handler returned with its text redacted, an `OK` one as
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
/// `tonic::Status` with its text redacted, an `OK` one as [`from_status`] reads it, a `CallError`
/// [`from_status`] built as that status, any other error as `CallError::from_boxed` recognises
/// it; the status's metadata screened by [`screened`].
///
/// A `CallError`'s message and details go through the app's redaction (`CallError::new`). A bare
/// `tonic::Status`, and the one a `CallError` [`from_status`] built carries, has its message and
/// the text in its details redacted ([`redacted_status`]) and its metadata left as the handler's
/// exact value.
pub(crate) fn render_as_is(err: BoxError, app: &AppHandle) -> Status {
    rendered(err, |call| call.__into_sent(app), |status| redacted_status(status, app))
}

/// [`render_as_is`], `sent` giving the `CallError` as it is sent and `scrubbed` a bare status as it
/// is sent.
fn rendered(err: BoxError, sent: impl FnOnce(CallError) -> CallError, scrubbed: impl FnOnce(Status) -> Status) -> Status {
    let status = match err.downcast::<Status>() {
        Ok(status) if status.code() != Code::Ok => scrubbed(*status),
        Ok(status) => to_status(&from_status(*status)),
        Err(err) => {
            let call = CallError::from_boxed(err);
            match carried(&call) {
                Some(status) => scrubbed(status.clone()),
                None => to_status(&sent(call)),
            }
        }
    };
    screened(status)
}

/// `status` with its message and the text in its details passed through the app's redaction, the
/// code and the metadata as they stand. A detail of a `google.rpc` detail type or a
/// `google.protobuf` type `to_status` writes is read by its schema, any other by its wire format
/// ([`redact_bytes`]), and details that are no `google.rpc.Status` the same way. A status nothing
/// in it changes is returned as it came.
fn redacted_status(status: Status, app: &AppHandle) -> Status {
    let redact = |text: &mut String| match app.redact_text(text) {
        Cow::Owned(replaced) => {
            *text = replaced;
            true
        }
        Cow::Borrowed(_) => false,
    };
    let mut changed = false;
    let mut message = status.message().to_owned();
    changed |= redact(&mut message);
    let mut details = Bytes::copy_from_slice(status.details());
    if !details.is_empty() {
        match tonic_types::Status::decode(details.clone()) {
            Ok(mut packed) => {
                let mut detail_changed = redact(&mut packed.message);
                for any in &mut packed.details {
                    detail_changed |= redact_any(any, &redact);
                }
                if detail_changed {
                    details = Bytes::from(packed.encode_to_vec());
                    changed = true;
                }
            }
            Err(_) => match redact_bytes(&details, &redact, 0) {
                Walked::Unchanged => {}
                Walked::Replaced(replaced) => {
                    details = Bytes::from(replaced);
                    changed = true;
                }
                Walked::Withheld => {
                    tracing::warn!(code = ?status.code(), "a status's details hold a registered secret in bytes that read as no text; they are not sent");
                    details = Bytes::new();
                    changed = true;
                }
            },
        }
    }
    if !changed {
        return status;
    }
    Status::with_details_and_metadata(status.code(), message, details, status.metadata().clone())
}

type Redact<'r> = &'r dyn Fn(&mut String) -> bool;

/// Redacts the strings of `any` by its type when it is one of the known ones, and through
/// [`redact_unknown`] otherwise, re-encoding it; whether it changed.
fn redact_any(any: &mut prost_types::Any, redact: Redact<'_>) -> bool {
    use tonic_types::pb::{
        BadRequest, DebugInfo, ErrorInfo, Help, LocalizedMessage, PreconditionFailure, QuotaFailure, RequestInfo, ResourceInfo,
    };
    let name = any.type_url.rsplit('/').next().unwrap_or_default();
    match name {
        "google.rpc.ErrorInfo" => redact_any_as(any, redact, |info: &mut ErrorInfo, redact| {
            redact(&mut info.reason) | redact(&mut info.domain) | redact_map(&mut info.metadata, redact)
        }),
        "google.rpc.DebugInfo" => redact_any_as(any, redact, |info: &mut DebugInfo, redact| {
            info.stack_entries.iter_mut().fold(redact(&mut info.detail), |changed, entry| redact(entry) | changed)
        }),
        "google.rpc.QuotaFailure" => redact_any_as(any, redact, |failure: &mut QuotaFailure, redact| {
            failure.violations.iter_mut().fold(false, |changed, violation| {
                redact(&mut violation.subject)
                    | redact(&mut violation.description)
                    | redact(&mut violation.api_service)
                    | redact(&mut violation.quota_metric)
                    | redact(&mut violation.quota_id)
                    | redact_map(&mut violation.quota_dimensions, redact)
                    | changed
            })
        }),
        "google.rpc.PreconditionFailure" => redact_any_as(any, redact, |failure: &mut PreconditionFailure, redact| {
            failure
                .violations
                .iter_mut()
                .fold(false, |changed, violation| {
                    redact(&mut violation.r#type) | redact(&mut violation.subject) | redact(&mut violation.description) | changed
                })
        }),
        "google.rpc.BadRequest" => redact_any_as(any, redact, |request: &mut BadRequest, redact| {
            request.field_violations.iter_mut().fold(false, |changed, violation| {
                let mut now = redact(&mut violation.field) | redact(&mut violation.description) | redact(&mut violation.reason);
                if let Some(localized) = &mut violation.localized_message {
                    now |= redact(&mut localized.locale) | redact(&mut localized.message);
                }
                now | changed
            })
        }),
        "google.rpc.RequestInfo" => {
            redact_any_as(any, redact, |info: &mut RequestInfo, redact| redact(&mut info.request_id) | redact(&mut info.serving_data))
        }
        "google.rpc.ResourceInfo" => redact_any_as(any, redact, |info: &mut ResourceInfo, redact| {
            redact(&mut info.resource_type) | redact(&mut info.resource_name) | redact(&mut info.owner) | redact(&mut info.description)
        }),
        "google.rpc.Help" => redact_any_as(any, redact, |help: &mut Help, redact| {
            help.links.iter_mut().fold(false, |changed, link| redact(&mut link.description) | redact(&mut link.url) | changed)
        }),
        "google.rpc.LocalizedMessage" => {
            redact_any_as(any, redact, |message: &mut LocalizedMessage, redact| redact(&mut message.locale) | redact(&mut message.message))
        }
        "google.protobuf.Value" => redact_any_as(any, redact, |value: &mut prost_types::Value, redact| redact_value(value, redact)),
        "google.protobuf.Struct" => {
            redact_any_as(any, redact, |value: &mut prost_types::Struct, redact| redact_fields(&mut value.fields, redact))
        }
        _ => redact_unknown(any, redact),
    }
}

/// Redacts `any` read through [`redact_bytes`], for a type the renderer has no schema for or a
/// known one whose bytes do not decode as it; whether it changed.
fn redact_unknown(any: &mut prost_types::Any, redact: Redact<'_>) -> bool {
    match redact_bytes(&any.value, redact, 0) {
        Walked::Unchanged => false,
        Walked::Replaced(replaced) => {
            any.value = replaced;
            true
        }
        Walked::Withheld => {
            tracing::warn!(type_url = %any.type_url, "a status detail holds a registered secret in bytes that read as no text; it is sent empty");
            any.value.clear();
            true
        }
    }
}

/// How deep [`redact_wire`] reads messages inside messages, protobuf's own default recursion limit.
const WIRE_DEPTH: usize = 100;

/// What redacting an encoded value leaves.
enum Walked {
    Unchanged,
    Replaced(Vec<u8>),
    /// A registered secret sits in bytes that read neither as a message nor as text.
    Withheld,
}

/// `bytes` of a type the renderer has no schema for, redacted: as an encoded message when a field
/// of that reading changes ([`redact_wire`]); otherwise as text when they are UTF-8 and either read
/// as no message or hold no control character but tab, CR and LF, as a string that happens to
/// read as a message does and an encoded message's tags and lengths almost never do; otherwise
/// withheld when they read as no message and a redaction of their lossy text would change it,
/// since a secret's bytes are contiguous in any encoding that carries it.
fn redact_bytes(bytes: &[u8], redact: Redact<'_>, depth: usize) -> Walked {
    let message = redact_wire(bytes, redact, depth);
    if let Some(Walked::Replaced(walked)) = message {
        return Walked::Replaced(walked);
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        if message.is_some() && text.chars().any(|c| c.is_control() && !matches!(c, '\t' | '\r' | '\n')) {
            return Walked::Unchanged;
        }
        let mut text = text.to_owned();
        return if redact(&mut text) { Walked::Replaced(text.into_bytes()) } else { Walked::Unchanged };
    }
    if message.is_none() && redact(&mut String::from_utf8_lossy(bytes).into_owned()) { Walked::Withheld } else { Walked::Unchanged }
}

/// `bytes` read as an encoded protobuf message, each length-delimited field redacted by
/// [`redact_bytes`] and its length prefix rewritten, every other field copied; `None` when the
/// bytes do not read as a message. A field that reads both as a message and as text is read as a
/// message first, so a nested message's lengths stay right; a withheld field is left out, which
/// the wire reads as the field unset.
fn redact_wire(mut bytes: &[u8], redact: Redact<'_>, depth: usize) -> Option<Walked> {
    use prost::encoding::{decode_varint, encode_varint};
    if depth == WIRE_DEPTH {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut changed = false;
    while !bytes.is_empty() {
        let field = bytes;
        let key = decode_varint(&mut bytes).ok()?;
        if key >> 3 == 0 || key >> 3 > (1 << 29) - 1 {
            return None;
        }
        let key_len = field.len() - bytes.len();
        match key & 7 {
            0 => {
                decode_varint(&mut bytes).ok()?;
            }
            1 | 5 => {
                let width = if key & 7 == 1 { 8 } else { 4 };
                bytes = bytes.get(width..)?;
            }
            2 => {
                let len = usize::try_from(decode_varint(&mut bytes).ok()?).ok()?;
                let inner = bytes.get(..len)?;
                bytes = &bytes[len..];
                match redact_bytes(inner, redact, depth + 1) {
                    Walked::Unchanged => {
                        out.extend_from_slice(&field[..key_len]);
                        encode_varint(inner.len() as u64, &mut out);
                        out.extend_from_slice(inner);
                    }
                    Walked::Replaced(replaced) => {
                        out.extend_from_slice(&field[..key_len]);
                        encode_varint(replaced.len() as u64, &mut out);
                        out.extend_from_slice(&replaced);
                        changed = true;
                    }
                    Walked::Withheld => changed = true,
                }
                continue;
            }
            _ => return None,
        }
        out.extend_from_slice(&field[..field.len() - bytes.len()]);
    }
    Some(if changed { Walked::Replaced(out) } else { Walked::Unchanged })
}

/// Decodes `any` as `M`, runs `walk` over it and re-encodes it when `walk` reports a change; an
/// `any` that does not decode as `M` is read as [`redact_unknown`] reads it.
fn redact_any_as<M: prost::Message + Default>(any: &mut prost_types::Any, redact: Redact<'_>, walk: impl FnOnce(&mut M, Redact<'_>) -> bool) -> bool {
    let Ok(mut message) = M::decode(any.value.as_slice()) else { return redact_unknown(any, redact) };
    let changed = walk(&mut message, redact);
    if changed {
        any.value = message.encode_to_vec();
    }
    changed
}

fn redact_map(map: &mut HashMap<String, String>, redact: Redact<'_>) -> bool {
    let mut changed = false;
    *map = std::mem::take(map)
        .into_iter()
        .map(|(mut key, mut value)| {
            changed |= redact(&mut key) | redact(&mut value);
            (key, value)
        })
        .collect();
    changed
}

fn redact_fields(fields: &mut std::collections::BTreeMap<String, prost_types::Value>, redact: Redact<'_>) -> bool {
    let mut changed = false;
    *fields = std::mem::take(fields)
        .into_iter()
        .map(|(mut key, mut value)| {
            changed |= redact(&mut key) | redact_value(&mut value, redact);
            (key, value)
        })
        .collect();
    changed
}

fn redact_value(value: &mut prost_types::Value, redact: Redact<'_>) -> bool {
    match &mut value.kind {
        Some(Kind::StringValue(text)) => redact(text),
        Some(Kind::ListValue(list)) => list.values.iter_mut().fold(false, |changed, item| redact_value(item, redact) | changed),
        Some(Kind::StructValue(map)) => redact_fields(&mut map.fields, redact),
        _ => false,
    }
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
            Detail::ErrorInfo { reason, domain, metadata, .. } => {
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
        let sent = rendered(Box::new(call), |call| call, |status| status);
        assert_eq!((sent.code(), sent.message()), (Code::Internal, OK_AS_ERROR));
    }

    /// A `CallError` shaped as `from_status` would build one from an `OK` status, kind `Internal`
    /// and the status's message, is still not sent as that status.
    #[test]
    fn an_ok_status_is_never_carried_as_an_error_s_status() {
        let call = CallError::new(ErrorKind::Internal, "done").with_source(Status::ok("done"));
        assert!(carried(&call).is_none());
        assert_eq!(rendered(Box::new(call), |call| call, |status| status).code(), Code::Internal);
    }

    /// An `OK` status returned bare as the error, by a handler or an error handler, is the same
    /// server bug.
    #[test]
    fn a_bare_ok_status_is_sent_as_internal() {
        let sent = rendered(Box::new(Status::ok("done")), |call| call, |status| status);
        assert_eq!((sent.code(), sent.message()), (Code::Internal, OK_AS_ERROR));
        assert_eq!(rendered(Box::new(Status::not_found("gone")), |call| call, |status| status).code(), Code::NotFound, "any other status as it stands");
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
        let sent = rendered(Box::new(call()), |call| call, |status| status);
        assert_eq!((sent.code(), sent.metadata().get("x-cursor").and_then(|value| value.to_str().ok())), (Code::FailedPrecondition, Some("7")));
        let renamed = call().with_grpc_code(Code::OutOfRange);
        assert!(carried(&renamed).is_none(), "an error naming another code is not the status");
        assert_eq!(rendered(Box::new(renamed), |call| call, |status| status).code(), Code::OutOfRange);
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

    /// A detail of a type with no schema here is redacted by its wire format: a string field and
    /// the strings of a nested message, every enclosing length rewritten, so it still decodes; a
    /// field of bytes that read as no text holding the secret is left out; bytes with no secret
    /// are unchanged. Planted break: the unknown arm answering `false`.
    #[test]
    fn a_detail_of_an_unknown_type_is_redacted_by_its_wire_format() {
        const SECRET: &str = "s3cr3t-token-xyz";
        let redact = |text: &mut String| {
            let replaced = text.replace(SECRET, "[redacted]");
            let changed = replaced != *text;
            *text = replaced;
            changed
        };
        let info = tonic_types::pb::ErrorInfo {
            reason: format!("key {SECRET} refused"),
            domain: "d".to_owned(),
            metadata: HashMap::from([("key".to_owned(), SECRET.to_owned())]),
        };
        let mut any = prost_types::Any { type_url: "type.googleapis.com/app.v1.KeyDetail".to_owned(), value: info.encode_to_vec() };
        assert!(redact_any(&mut any, &redact));
        let read = tonic_types::pb::ErrorInfo::decode(any.value.as_slice()).expect("the detail still decodes");
        assert_eq!((read.reason.as_str(), read.metadata["key"].as_str()), ("key [redacted] refused", "[redacted]"));

        let mut binary = vec![0xff, 0xfe];
        binary.extend_from_slice(SECRET.as_bytes());
        let mut outer = vec![0x0a, u8::try_from(binary.len()).expect("short")];
        outer.extend_from_slice(&binary);
        outer.extend_from_slice(&[0x10, 0x05]);
        assert!(matches!(redact_bytes(&outer, &redact, 0), Walked::Replaced(out) if out == [0x10, 0x05]));

        let plain = tonic_types::pb::ErrorInfo { reason: "r".to_owned(), ..Default::default() }.encode_to_vec();
        assert!(matches!(redact_bytes(&plain, &redact, 0), Walked::Unchanged));
        assert!(matches!(redact_bytes(format!("raw {SECRET}").as_bytes(), &redact, 0), Walked::Replaced(out) if out == b"raw [redacted]"));
    }

    /// Every string of every `google.rpc` detail type is redacted: each type built with an
    /// exhaustive literal, so a field tonic-types adds fails this test's compile until it is
    /// handled, and the redacted detail decodes with no trace of the secret. Planted break: any
    /// one field left out of `redact_any`.
    #[test]
    fn every_string_of_a_google_rpc_detail_is_redacted() {
        use tonic_types::pb::{
            BadRequest, DebugInfo, ErrorInfo, Help, LocalizedMessage, PreconditionFailure, QuotaFailure, RequestInfo, ResourceInfo,
            bad_request, help, precondition_failure, quota_failure,
        };
        const SECRET: &str = "s3cr3t-token-xyz";
        let redact = |text: &mut String| {
            let replaced = text.replace(SECRET, "[redacted]");
            let changed = replaced != *text;
            *text = replaced;
            changed
        };
        let s = || format!("a {SECRET} b");
        let map = || HashMap::from([(s(), s())]);
        let localized = || LocalizedMessage { locale: s(), message: s() };
        type Read = fn(&[u8]) -> String;
        let details: Vec<(&str, Vec<u8>, Read)> = vec![
            ("ErrorInfo", ErrorInfo { reason: s(), domain: s(), metadata: map() }.encode_to_vec(), |b| format!("{:?}", ErrorInfo::decode(b))),
            ("DebugInfo", DebugInfo { stack_entries: vec![s()], detail: s() }.encode_to_vec(), |b| format!("{:?}", DebugInfo::decode(b))),
            (
                "QuotaFailure",
                QuotaFailure {
                    violations: vec![quota_failure::Violation {
                        subject: s(),
                        description: s(),
                        api_service: s(),
                        quota_metric: s(),
                        quota_id: s(),
                        quota_dimensions: map(),
                        quota_value: 1,
                        future_quota_value: Some(2),
                    }],
                }
                .encode_to_vec(),
                |b| format!("{:?}", QuotaFailure::decode(b)),
            ),
            (
                "PreconditionFailure",
                PreconditionFailure { violations: vec![precondition_failure::Violation { r#type: s(), subject: s(), description: s() }] }
                    .encode_to_vec(),
                |b| format!("{:?}", PreconditionFailure::decode(b)),
            ),
            (
                "BadRequest",
                BadRequest {
                    field_violations: vec![bad_request::FieldViolation {
                        field: s(),
                        description: s(),
                        reason: s(),
                        localized_message: Some(localized()),
                    }],
                }
                .encode_to_vec(),
                |b| format!("{:?}", BadRequest::decode(b)),
            ),
            ("RequestInfo", RequestInfo { request_id: s(), serving_data: s() }.encode_to_vec(), |b| format!("{:?}", RequestInfo::decode(b))),
            (
                "ResourceInfo",
                ResourceInfo { resource_type: s(), resource_name: s(), owner: s(), description: s() }.encode_to_vec(),
                |b| format!("{:?}", ResourceInfo::decode(b)),
            ),
            ("Help", Help { links: vec![help::Link { description: s(), url: s() }] }.encode_to_vec(), |b| format!("{:?}", Help::decode(b))),
            ("LocalizedMessage", localized().encode_to_vec(), |b| format!("{:?}", LocalizedMessage::decode(b))),
        ];
        for (name, value, read) in details {
            let mut any = prost_types::Any { type_url: format!("type.googleapis.com/google.rpc.{name}"), value };
            assert!(redact_any(&mut any, &redact), "{name}");
            let read = read(&any.value);
            assert!(read.starts_with("Ok(") && !read.contains(SECRET), "{name}: {read}");
        }
    }

    /// A string that also reads as an encoded message is redacted as text when no field of that
    /// reading holds the secret whole. Planted break: the walker's answer taken whenever the bytes
    /// read as a message.
    #[test]
    fn a_string_that_reads_as_a_message_is_redacted_as_text() {
        const SECRET: &str = "s3cr3t-token-xyz";
        let redact = |text: &mut String| {
            let replaced = text.replace(SECRET, "[redacted]");
            let changed = replaced != *text;
            *text = replaced;
            changed
        };
        // Field 15 of 32 bytes ending in "s3c", then field 14 (`r`) of 51 bytes (`3`) holding the
        // rest of the secret: every byte printable, and no field holds the secret whole.
        let text = format!("z {}{SECRET}{}", "A".repeat(29), ".".repeat(40));
        assert!(redact_wire(text.as_bytes(), &redact, 0).is_some(), "the text reads as a message");
        let Walked::Replaced(out) = redact_bytes(text.as_bytes(), &redact, 0) else { panic!("{text:?} not redacted") };
        assert_eq!(String::from_utf8(out).expect("text"), text.replace(SECRET, "[redacted]"));
    }

    /// A boxed status keeps its code through the helper and through the renderer; `from_boxed`
    /// alone names no tonic type and answers it `Internal`.
    #[test]
    fn a_boxed_status_keeps_its_code_through_the_helper_and_the_renderer() {
        let boxed = || -> BoxError { Box::new(Status::failed_precondition("stale")) };
        assert_eq!(CallError::from_boxed(boxed()).grpc_code(), None);
        assert_eq!(CallError::from_boxed(boxed()).kind(), ErrorKind::Internal);
        let call = call_error(boxed());
        assert_eq!((call.kind(), call.grpc_code(), call.message()), (ErrorKind::BadRequest, Some(Code::FailedPrecondition), "stale"));
        assert_eq!(rendered(boxed(), |call| call, |status| status).code(), Code::FailedPrecondition);
        assert_eq!(call_error("plain".into()).kind(), ErrorKind::Internal);
    }
}
