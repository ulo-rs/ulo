//! HTTP's rendering of the one error model (transports DESIGN §2.4, §3.6): RFC 9457
//! `application/problem+json`, `type` `about:blank`, `title` the status phrase, `status` the code,
//! `detail` the public message, `details` as an extension member.

use std::time::Duration;

use http::header::{ACCEPT, ALLOW, CONNECTION, CONTENT_TYPE, HeaderValue, RETRY_AFTER, WWW_AUTHENTICATE};
use http::StatusCode;
use serde::Serialize;
use ulo::{AppHandle, BoxError, CancelReason, ExecutionRef};
use ulo_transport::{CallError, Details, ErrorKind, ExtractError};

use crate::backend::HttpConfig;
use crate::body::HttpBody;
use crate::miss::{MethodNotAllowed, NotAcceptable};
use crate::response::Response;
use crate::routing::Routing;
use crate::sse::{Event, EventName};

/// The canonical status of a kind. `Timeout` is 504, the HTTP mapping `google.rpc.Code` documents
/// for DEADLINE_EXCEEDED (`code.proto`), which grpc-gateway follows; 408 means the server gave up
/// waiting for the client's request.
pub(crate) fn status_of(kind: ErrorKind) -> StatusCode {
    match kind {
        ErrorKind::BadRequest => StatusCode::BAD_REQUEST,
        ErrorKind::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorKind::Forbidden => StatusCode::FORBIDDEN,
        ErrorKind::NotFound => StatusCode::NOT_FOUND,
        ErrorKind::Conflict => StatusCode::CONFLICT,
        ErrorKind::Unprocessable => StatusCode::UNPROCESSABLE_ENTITY,
        ErrorKind::TooManyRequests => StatusCode::TOO_MANY_REQUESTS,
        ErrorKind::Timeout => StatusCode::GATEWAY_TIMEOUT,
        ErrorKind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        ErrorKind::Unimplemented => StatusCode::NOT_IMPLEMENTED,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

/// Sends a [`CallError`] over HTTP with a status of its own in place of the one its kind maps to,
/// as `with_grpc_code` does for gRPC: a `Conflict` that names a precondition the request failed
/// writes `.with_http_status(StatusCode::PRECONDITION_FAILED)`. The kind still decides every other
/// transport's rendering, the RPC and WebSocket envelopes' `kind`, a WebSocket connect refusal's
/// close code and the gRPC status, so the error keeps one meaning across transports.
///
/// The problem document carries the status in its `status` and `title`, and the headers follow
/// the status, not the kind: a 401 carries `WWW-Authenticate`, and a 429 or 503 carries
/// `Retry-After` from a `RetryAfter` detail. A status outside 400 to 599 is not an error's and is
/// ignored, logged at `warn` when the error is rendered: a 1xx cannot end a response, and a 2xx or
/// 3xx would read as success.
///
/// ```
/// use ulo_http::{CallError, ErrorKind, StatusCode, WithHttpStatus};
///
/// let error = CallError::new(ErrorKind::Conflict, "the version changed").with_http_status(StatusCode::PRECONDITION_FAILED);
/// assert_eq!(error.kind(), ErrorKind::Conflict);
/// ```
pub trait WithHttpStatus {
    fn with_http_status(self, status: StatusCode) -> Self;
}

impl WithHttpStatus for CallError {
    fn with_http_status(self, status: StatusCode) -> Self {
        self.__with_http_status(status.as_u16())
    }
}

/// The status `error` is rendered with: the one [`WithHttpStatus`] set when it is an error's,
/// otherwise its kind's.
pub(crate) fn status_for(error: &CallError) -> StatusCode {
    own_status(error).unwrap_or_else(|| status_of(error.kind()))
}

/// The status [`WithHttpStatus`] set on `error`, when it is 400 to 599. One outside that range is
/// logged at `warn` and read as unset.
fn own_status(error: &CallError) -> Option<StatusCode> {
    let written = error.__http_status()?;
    match StatusCode::from_u16(written) {
        Ok(status) if status.is_client_error() || status.is_server_error() => Some(status),
        _ => {
            tracing::warn!(status = written, kind = %error.kind(), "an error's HTTP status is not 400 to 599; it is sent with its kind's");
            None
        }
    }
}

/// The problem document for `error`, with the status [`status_for`] gives. A 401 carries
/// `WWW-Authenticate`, the error's own challenge or the server's default; a 429 carries
/// `Retry-After` from a `RetryAfter` detail, and a 503 does when one is present.
pub(crate) fn problem(error: &CallError, config: &HttpConfig) -> Response {
    document(error, status_for(error), config)
}

/// An error no handler claimed, as a response: `Timeout` when `exec`'s cancel reason is
/// `Deadline`, otherwise as [`render_as_is`] renders it.
pub(crate) fn render_error(err: BoxError, exec: &ExecutionRef, config: &HttpConfig) -> Response {
    if exec.cancel_reason() == Some(CancelReason::Deadline) {
        return problem(&timed_out(), config);
    }
    render_as_is(err, config)
}

/// An error no handler claimed, rendered as itself whatever the execution's cancel reason: what a
/// route timeout's error handlers return, which is the `Timeout` it offered or their reshaping of
/// it.
///
/// `CallError::from_boxed`, with four statuses no kind has, each while the error is still of
/// kind `BadRequest`: 405 with `Allow` for a `MethodNotAllowed`, 406 for a `NotAcceptable`, and
/// 413 and 415 for an `ExtractError`'s `TooLarge` and `UnsupportedMediaType`. Each is recognised
/// as the source of a `CallError`, and all but `NotAcceptable` bare as well; an error handler that
/// reshaped it to another kind is rendered by that kind, and one that set a status through
/// [`WithHttpStatus`] by that status. A 415 carries `Accept` with the media type the extractor
/// expected (RFC 9110 §15.5.16).
pub(crate) fn render_as_is(err: BoxError, config: &HttpConfig) -> Response {
    let error = match err.downcast::<MethodNotAllowed>() {
        Ok(refused) => (*refused).into_error(),
        Err(err) => CallError::from_boxed(err),
    };
    if error.kind() != ErrorKind::BadRequest {
        return problem(&error, config);
    }
    if let Some(status) = own_status(&error) {
        return document(&error, status, config);
    }
    if let Some(refused) = error.source_as::<MethodNotAllowed>() {
        return method_not_allowed(&error, refused, config);
    }
    if error.source_as::<NotAcceptable>().is_some() {
        return document(&error, StatusCode::NOT_ACCEPTABLE, config);
    }
    match error.source_as::<ExtractError>() {
        Some(ExtractError::TooLarge { .. }) => document(&error, StatusCode::PAYLOAD_TOO_LARGE, config),
        Some(ExtractError::UnsupportedMediaType { expected, .. }) => {
            let mut response = document(&error, StatusCode::UNSUPPORTED_MEDIA_TYPE, config);
            if let Ok(accept) = HeaderValue::from_str(expected) {
                response.headers_mut().insert(ACCEPT, accept);
            }
            response
        }
        _ => document(&error, StatusCode::BAD_REQUEST, config),
    }
}

/// A multipart read error a handler returned, as the `ExtractError` it is: `TooLarge` when the read
/// failed because the body passed the route's limit, `Malformed` (400) for any other `multer`
/// error, a broken boundary or a cut-off part header among them. Multer wraps a stream's error as
/// the source of its own `StreamReadFailed`, which a handler propagating it with `?` returns, and
/// the classification reads the error itself rather than its sources. An error that is not
/// multer's is returned as it came. Applied to every error a handler returns, so the error
/// handlers see the `ExtractError`.
pub(crate) fn from_multipart(err: BoxError, app: &AppHandle) -> BoxError {
    let read = match err.downcast::<multer::Error>() {
        Ok(read) => read,
        Err(err) => return err,
    };
    if let multer::Error::StreamReadFailed(source) = &*read
        && let Some(&ExtractError::TooLarge { param, limit }) = source.downcast_ref::<ExtractError>()
    {
        return Box::new(ExtractError::TooLarge { param, limit });
    }
    Box::new(ExtractError::Malformed { param: "body", source: app.redact(read) })
}

/// What every refusal of a call because the server is draining or closing says, on every
/// transport.
pub(crate) const SHUTTING_DOWN: &str = ulo_transport::SHUTDOWN_MESSAGE;

/// 503 with `Retry-After` and `Connection: close`: a request arriving once `Execution::open` is
/// refused, during the drain, and a call still running when the drain's end abandons it.
pub(crate) fn draining(config: &HttpConfig) -> Response {
    let error = CallError::new(ErrorKind::Unavailable, SHUTTING_DOWN);
    let mut response = refusal(&error, config);
    response.headers_mut().insert(CONNECTION, HeaderValue::from_static("close"));
    response
}

/// 503 with `Retry-After`: a request an embedding received before the app listened (R14).
pub(crate) fn not_listening(config: &HttpConfig) -> Response {
    refusal(&CallError::new(ErrorKind::Unavailable, "the application is not yet listening"), config)
}

/// 400: a path holding a `.` or `..` segment, refused before the pre-dispatch stage and routing.
pub(crate) fn dot_segment(config: &HttpConfig) -> Response {
    problem(&CallError::new(ErrorKind::BadRequest, "the path holds a `.` or `..` segment"), config)
}

/// 400: a path holding a `\`, raw or as `%5C`, refused where a dot segment is.
pub(crate) fn backslash(config: &HttpConfig) -> Response {
    problem(&CallError::new(ErrorKind::BadRequest, "the path holds a `\\`, raw or as `%5C`"), config)
}

/// 503 with `Retry-After`: over the server's in-flight bound.
pub(crate) fn shed(config: &HttpConfig) -> Response {
    refusal(&CallError::new(ErrorKind::Unavailable, "the server is at its limit of requests in flight"), config)
}

/// An error after an SSE stream began, in its mid-stream form: an event named `error` whose data
/// is the envelope `{"kind", "message", "details"}`, after which the stream ends.
pub(crate) fn late_event(error: &CallError) -> Event {
    let envelope = Envelope { kind: error.kind().as_str(), message: error.message(), details: error.details() };
    let data = serde_json::to_string(&envelope).unwrap_or_else(|failure| {
        tracing::warn!(error = %failure, "the details of an SSE error event could not be encoded; they are left out");
        let bare = Details::default();
        let envelope = Envelope { details: &bare, ..envelope };
        serde_json::to_string(&envelope).unwrap_or_default()
    });
    Event::default().event(EventName::error()).data(data)
}

/// `Timeout`, for an execution cancelled by its deadline: the error it ends with is whatever the
/// cancellation interrupted, and the reason lives on the execution.
pub(crate) fn timed_out() -> CallError {
    CallError::new(ErrorKind::Timeout, "the request did not complete within its time limit")
}

/// 405 as problem details, with the `Allow` RFC 9110 requires.
fn method_not_allowed(error: &CallError, refused: &MethodNotAllowed, config: &HttpConfig) -> Response {
    let mut response = document(error, StatusCode::METHOD_NOT_ALLOWED, config);
    response.headers_mut().insert(ALLOW, refused.allow().clone());
    response
}

/// A 503 that always carries `Retry-After`, the server's `shed_retry_after`, and
/// `Routing::Unrouted`: each refusal is given before routing.
fn refusal(error: &CallError, config: &HttpConfig) -> Response {
    let mut response = document(error, StatusCode::SERVICE_UNAVAILABLE, config);
    response.headers_mut().insert(RETRY_AFTER, retry_after(config.shed_retry_after));
    response.extensions_mut().insert(Routing::Unrouted);
    response
}

fn document(error: &CallError, status: StatusCode, config: &HttpConfig) -> Response {
    let problem = Problem { kind: "about:blank", title: title(status), status: status.as_u16(), detail: error.message(), details: error.details() };
    let body = serde_json::to_vec(&problem).unwrap_or_else(|failure| {
        tracing::warn!(error = %failure, "the details of a problem document could not be encoded; they are left out");
        let bare = Details::default();
        serde_json::to_vec(&Problem { details: &bare, ..problem }).unwrap_or_default()
    });
    let mut response = Response::new(HttpBody::from_bytes(body));
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/problem+json"));
    if status == StatusCode::UNAUTHORIZED {
        let challenge = error.challenge().unwrap_or(&*config.challenge);
        let value = HeaderValue::from_str(challenge).unwrap_or_else(|_| {
            tracing::warn!(challenge, "a `WWW-Authenticate` challenge is not a valid header value; `Bearer` is sent instead");
            HeaderValue::from_static("Bearer")
        });
        headers.insert(WWW_AUTHENTICATE, value);
    }
    if status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE {
        if let Some(after) = error.details().retry_after() {
            headers.insert(RETRY_AFTER, retry_after(after));
        }
    }
    response
}

/// `Retry-After` as delay-seconds (RFC 9110 §10.2.3), a fraction of a second rounded up so a client
/// never retries early.
fn retry_after(after: Duration) -> HeaderValue {
    let seconds = after.as_secs() + u64::from(after.subsec_nanos() > 0);
    HeaderValue::from(seconds)
}

/// The status phrase RFC 9110 recommends, which RFC 9457 makes the `title` of an `about:blank`
/// problem; `None` for a status with no registered phrase, whose document carries no `title`. The
/// `http` crate's phrases predate RFC 9110 for 413 and 422.
fn title(status: StatusCode) -> Option<&'static str> {
    match status.as_u16() {
        413 => Some("Content Too Large"),
        422 => Some("Unprocessable Content"),
        _ => status.canonical_reason(),
    }
}

#[derive(Clone, Copy, Serialize)]
struct Problem<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<&'static str>,
    status: u16,
    detail: &'a str,
    #[serde(skip_serializing_if = "no_details")]
    details: &'a Details,
}

#[derive(Clone, Copy, Serialize)]
struct Envelope<'a> {
    kind: &'static str,
    message: &'a str,
    details: &'a Details,
}

fn no_details(details: &&Details) -> bool {
    details.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A status [`WithHttpStatus`] set outside 400 to 599 is read as unset, so the statuses no
    /// kind has still apply to the error it was set on.
    #[test]
    fn a_status_no_error_has_leaves_the_extract_error_s_own_status() {
        let too_large = || CallError::from(ExtractError::TooLarge { param: "body", limit: 8 });
        let config = HttpConfig::default();
        let ignored = render_as_is(Box::new(too_large().with_http_status(StatusCode::OK)), &config);
        assert_eq!(ignored.status(), StatusCode::PAYLOAD_TOO_LARGE, "a 200 is not an error's status");
        let named = render_as_is(Box::new(too_large().with_http_status(StatusCode::CONFLICT)), &config);
        assert_eq!(named.status(), StatusCode::CONFLICT, "an error's own status wins over 413");
    }

    /// A status with no registered phrase is sent with no `title` rather than an empty one.
    #[test]
    fn a_status_with_no_phrase_has_no_title() {
        let status = StatusCode::from_u16(499).expect("499 is a status code");
        let response = problem(&CallError::new(ErrorKind::Conflict, "gone").with_http_status(status), &HttpConfig::default());
        assert_eq!(response.status(), status);
        let body = futures_util::FutureExt::now_or_never(http_body_util::BodyExt::collect(response.into_body()))
            .expect("the document is buffered")
            .expect("the body reads")
            .to_bytes();
        let document: serde_json::Value = serde_json::from_slice(&body).expect("a JSON document");
        assert_eq!(document.get("title"), None, "{document}");
        assert_eq!(document["status"], 499);
    }
}
