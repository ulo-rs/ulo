//! W3C Trace Context on a gRPC call (integrations §6.2, I6, I9): extracted from the request's
//! metadata in the dispatcher, before the execution opens, and injected into an outgoing
//! request's metadata by `outgoing`. HTTP/2 headers are the metadata, so both read and write the
//! `traceparent` and `tracestate` header fields. Also the rule by which a call's status marks its
//! server span as failed.

use http::HeaderMap;
use tonic::Code;
use tonic::metadata::{AsciiMetadataValue, MetadataMap};
use tracing::Span;
use ulo::{TraceContext, TraceOrigin, TraceState};
use ulo_transport::span;

/// The header a context is read from; a call without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";

/// The context the call's `traceparent` and `tracestate` fields carry, every field under each
/// name, through `TraceContext::extract`; `None` when there is none to continue, which never fails
/// the call (H4).
///
/// A `traceparent` field that is not visible ASCII is off the grammar, so the call carries no
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

/// Refuses a request whose caller set a header `outgoing` writes itself: `traceparent`, or
/// `tracestate` when `context` is `Received`. `INVALID_ARGUMENT`, as the RPC client's
/// `reserved_header` refusal is `BadRequest`; the status message carries the reason.
pub(crate) fn reserved(context: &TraceContext, metadata: &MetadataMap) -> Result<(), tonic::Status> {
    if metadata.contains_key(TRACEPARENT) {
        return Err(tonic::Status::invalid_argument(format!(
            "reserved_header: the call set `{TRACEPARENT}` in its metadata, and `outgoing` reserves that header on an execution carrying a trace context, writing it from that context"
        )));
    }
    if context.origin() == TraceOrigin::Received && metadata.contains_key(TRACESTATE) {
        return Err(tonic::Status::invalid_argument(format!(
            "reserved_header: the call set `{TRACESTATE}` in its metadata within an execution forwarding the context it received, which `outgoing` passes on unchanged; adding a vendor entry needs an exporter, a `TraceSource` such as `ulo-otel`'s, to originate the execution's context"
        )));
    }
    Ok(())
}

/// Writes `context`, the calling execution's, into an outgoing request's `metadata` as
/// `traceparent` and `tracestate`. `outgoing` has refused a caller's `traceparent`, and a
/// caller's `tracestate` on a `Received` context, through [`reserved`] first.
///
/// A `Received` context is forwarded as it arrived, both headers' text, which W3C Trace Context
/// §3.4 requires of a `traceparent` passed through unchanged. Into an `Originated` context the
/// caller's own `tracestate` entries are merged as §3.5 has a vendor add its own, by
/// `TraceState::prepend`.
///
/// A context whose `traceparent` text cannot travel as a metadata value, one received over a
/// transport that carries more than visible ASCII, is not written, and neither is a caller's: the
/// call then carries no context, and the next hop that participates starts a trace.
pub(crate) fn inject(context: &TraceContext, metadata: &mut MetadataMap) {
    let tracestate = match context.origin() {
        TraceOrigin::Received => context.tracestate(),
        TraceOrigin::Originated => {
            let caller: Option<Vec<&str>> = metadata.get_all(TRACESTATE).iter().map(|value| value.to_str().ok()).collect();
            let caller = caller.map(TraceState::parse).unwrap_or_default();
            let merged = context.state().prepend(&caller);
            (!merged.is_empty()).then(|| merged.to_string())
        }
    };
    metadata.remove(TRACEPARENT);
    metadata.remove(TRACESTATE);
    let Ok(traceparent) = AsciiMetadataValue::try_from(context.traceparent()) else {
        tracing::warn!("the calling execution's `traceparent` is not a valid metadata value; the call carries no trace context");
        return;
    };
    metadata.insert(TRACEPARENT, traceparent);
    // A list off the header grammar is discarded without affecting `traceparent` (§3.3).
    if let Some(tracestate) = tracestate.and_then(|text| AsciiMetadataValue::try_from(text).ok()) {
        metadata.insert(TRACESTATE, tracestate);
    }
}

/// Records `otel.status_code` as an error on a server span answering `code`, by OpenTelemetry's
/// gRPC conventions for a server: `UNKNOWN`, `DEADLINE_EXCEEDED`, `UNIMPLEMENTED`, `INTERNAL`,
/// `UNAVAILABLE` and `DATA_LOSS` are the server's failures, and every other code, a caller's
/// mistake or a refusal it can act on, leaves the status unset, as a 4xx does on HTTP.
pub(crate) fn record_status(call_span: &Span, code: Code) {
    if matches!(code, Code::Unknown | Code::DeadlineExceeded | Code::Unimplemented | Code::Internal | Code::Unavailable | Code::DataLoss) {
        call_span.record(span::OTEL_STATUS_CODE, span::STATUS_ERROR);
    }
}
