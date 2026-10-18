//! W3C Trace Context on an RPC call (integrations §6.2, §6.3, I6, I8, I9): extracted from the
//! frame's `h`, or the broker's own headers, before the execution opens, an event through the same
//! path as a request; injected into an outgoing frame's headers by the client on every shape,
//! events included. The client's refusal of a caller's `traceparent`, and of its `tracestate` beside
//! a received context, is `Spec::reserved`'s; what reaches [`inject`] has passed it. Also the rule
//! by which a call's error marks its span as failed.

use tracing::Span;
use ulo::{TraceContext, TraceOrigin, TraceState};
use ulo_transport::{ErrorKind, span};

use crate::transport::CallHeaders;

/// The header a context is read from; a call without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";

/// The context `headers`' `traceparent` and `tracestate` carry, every value under each name,
/// through `TraceContext::extract`; `None` when there is none to continue, which never fails the
/// call (H4).
///
/// The names are matched without regard to case, as W3C Trace Context matches them: a broker's
/// headers keep the case a producer wrote, and a `Traceparent` beside a `traceparent` is two fields,
/// which the specification makes a restart.
pub(crate) fn extract(headers: &CallHeaders) -> Option<TraceContext> {
    TraceContext::extract(values(headers, TRACEPARENT), values(headers, TRACESTATE))
}

fn values<'a>(headers: &'a CallHeaders, name: &'static str) -> impl Iterator<Item = &'a str> + 'a {
    headers.iter().filter(move |(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value)
}

/// Writes `context` into an outgoing frame's `headers`: `traceparent` from
/// `TraceContext::traceparent`; for a `TraceOrigin::Received` context `tracestate` as received,
/// from `TraceContext::tracestate`; for an `Originated` one, the caller's own `tracestate`, if
/// `headers` holds one, merged into the context's list by `TraceState::prepend` and written in
/// its place.
///
/// A caller's `tracestate` off the grammar is dropped, logged at `warn`, and the context's list is
/// written alone: §3.3 discards an invalid list whole, and a bad list never affects `traceparent`.
pub(crate) fn inject(headers: &mut CallHeaders, context: &TraceContext) {
    let tracestate = match context.origin() {
        TraceOrigin::Received => context.tracestate(),
        TraceOrigin::Originated => {
            let caller = TraceState::parse(values(headers, TRACESTATE));
            let written = values(headers, TRACESTATE).any(|field| !field.trim_matches([' ', '\t', ',']).is_empty());
            if caller.is_empty() && written {
                tracing::warn!("a caller's `tracestate` is off the W3C Trace Context grammar; it was dropped and the execution's list sent alone");
            }
            let merged = context.state().prepend(&caller);
            (!merged.is_empty()).then(|| merged.to_string())
        }
    };
    headers.entries.retain(|(name, _)| !name.eq_ignore_ascii_case(TRACEPARENT) && !name.eq_ignore_ascii_case(TRACESTATE));
    headers.insert(TRACEPARENT, context.traceparent());
    if let Some(tracestate) = tracestate {
        headers.insert(TRACESTATE, tracestate);
    }
}

/// Records `otel.status_code` as an error on a call's span when the call fails with `kind`, a kind
/// HTTP answers with a 5xx: the server's own failure, its deadline passing included. A caller's
/// mistake or a refusal it can act on, the kinds HTTP answers with a 4xx, leaves the status unset.
/// An event's handler, a consumer span, follows the same rule.
pub(crate) fn record_failure(call_span: &Span, kind: ErrorKind) {
    if !matches!(
        kind,
        ErrorKind::BadRequest
            | ErrorKind::Unauthorized
            | ErrorKind::Forbidden
            | ErrorKind::NotFound
            | ErrorKind::Conflict
            | ErrorKind::Unprocessable
            | ErrorKind::TooManyRequests
    ) {
        call_span.record(span::OTEL_STATUS_CODE, span::STATUS_ERROR);
    }
}
