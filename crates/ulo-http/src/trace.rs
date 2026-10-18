//! W3C Trace Context on an HTTP request (integrations §6.2, I9): extracted where `AppService`
//! opens the execution, for the served server and the embedding alike. The framework has no HTTP
//! client, so nothing is injected here. Also the rule by which a response's status marks its
//! server span as failed.

use http::{HeaderMap, StatusCode};
use tracing::Span;
use ulo::TraceContext;
use ulo_transport::span;

/// The header a context is read from; a call without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";

/// The context the request's `traceparent` and `tracestate` fields carry, every field under each
/// name, through `TraceContext::extract`; `None` when there is none to continue, which never fails
/// the request (H4).
///
/// A `traceparent` field that is not visible ASCII is off the grammar, so the request carries no
/// context. A `tracestate` field that is not visible ASCII discards the whole list, as any member
/// off the grammar does, and leaves the `traceparent` standing.
pub(crate) fn extract(headers: &HeaderMap) -> Option<TraceContext> {
    let traceparent = texts(headers, TRACEPARENT)?;
    let tracestate = texts(headers, TRACESTATE).unwrap_or_default();
    TraceContext::extract(traceparent, tracestate)
}

/// Every value under `name`, or `None` when one of them is not text a header carries.
fn texts<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<Vec<&'a str>> {
    headers.get_all(name).iter().map(|value| value.to_str().ok()).collect()
}

/// Records `otel.status_code` as an error on a server span answering `status` when it is a 5xx;
/// a 4xx is the client's mistake and leaves the status unset, as OpenTelemetry's HTTP conventions
/// have a server span (integrations §6.4).
pub(crate) fn record_status(call_span: &Span, status: StatusCode) {
    if status.is_server_error() {
        call_span.record(span::OTEL_STATUS_CODE, span::STATUS_ERROR);
    }
}
