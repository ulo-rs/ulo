//! W3C Trace Context on an HTTP request (integrations §6.2, I9): extracted where `AppService`
//! opens the execution, for the served server and the embedding alike. The framework has no HTTP
//! client, so nothing is injected here.

use http::HeaderMap;
use ulo::TraceContext;

/// The header a context is read from; a call without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";

/// The context the request's `traceparent` and `tracestate` fields carry, every field under each
/// name, through `TraceContext::extract`; `None` when there is none to continue, which never fails
/// the request (H4).
pub(crate) fn extract(headers: &HeaderMap) -> Option<TraceContext> {
    let _ = headers;
    todo!("O1: extraction from `traceparent` and `tracestate`")
}
