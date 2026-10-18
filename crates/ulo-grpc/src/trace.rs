//! W3C Trace Context on a gRPC call (integrations §6.2, I6, I9): extracted from the request's
//! metadata in the dispatcher, before the execution opens, and injected into an outgoing
//! request's metadata by `outgoing`. HTTP/2 headers are the metadata, so both read and write the
//! `traceparent` and `tracestate` header fields.

use http::HeaderMap;
use tonic::metadata::MetadataMap;
use ulo::TraceContext;

/// The header a context is read from; a call without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";

/// The context the call's `traceparent` and `tracestate` fields carry, every field under each
/// name, through `TraceContext::extract`; `None` when there is none to continue, which never fails
/// the call (H4).
pub(crate) fn extract(headers: &HeaderMap) -> Option<TraceContext> {
    let _ = headers;
    todo!("O1: extraction from `traceparent` and `tracestate`")
}

/// Writes `context`, the calling execution's, into an outgoing request's `metadata` as
/// `traceparent` and `tracestate`, by `TraceContext::traceparent` and `TraceContext::tracestate`,
/// in place of any the caller set.
pub(crate) fn inject(context: &TraceContext, metadata: &mut MetadataMap) {
    let _ = (context, metadata);
    todo!("O1: injection into the outgoing request's metadata")
}
