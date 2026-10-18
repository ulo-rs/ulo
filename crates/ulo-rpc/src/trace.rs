//! W3C Trace Context on an RPC call (integrations §6.2, §6.3, I6, I8, I9): extracted from the
//! frame's `h`, or the broker's own headers, before the execution opens, an event through the same
//! path as a request; injected into an outgoing frame's headers by the client on every shape,
//! events included. The client's refusal of a caller's `traceparent`, and of its `tracestate` beside
//! a received context, is `Spec::reserved`'s; what reaches [`inject`] has passed it.

use ulo::TraceContext;

use crate::transport::CallHeaders;

/// The header a context is read from; a call without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";

/// The context `headers`' `traceparent` and `tracestate` carry, every value under each name,
/// through `TraceContext::extract`; `None` when there is none to continue, which never fails the
/// call (H4).
pub(crate) fn extract(headers: &CallHeaders) -> Option<TraceContext> {
    let _ = headers;
    todo!("O1: extraction from `traceparent` and `tracestate`")
}

/// Writes `context` into an outgoing frame's `headers`: `traceparent` from
/// `TraceContext::traceparent`; for a `TraceOrigin::Received` context `tracestate` as received,
/// from `TraceContext::tracestate`; for an `Originated` one, the caller's own `tracestate`, if
/// `headers` holds one, merged into the context's list by `TraceState::prepend` and written in
/// its place.
pub(crate) fn inject(headers: &mut CallHeaders, context: &TraceContext) {
    let _ = (headers, context);
    todo!("O1: injection into the frame's headers, `tracestate` merged by W3C §3.5 when `Originated`")
}
