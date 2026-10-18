//! W3C Trace Context on a WebSocket connection (integrations §6.2, I9): extracted from the
//! handshake's headers into the connect phase's execution. The connection keeps the connect
//! execution's context and opens each later execution, a message's, a hand-written gateway's and
//! the terminal `on_disconnect`, with it as the inbound context, through
//! `Connection::exec_options`. The framework has no WebSocket client, so nothing is injected here.
//! Also the rule by which a connect's refusal or a message's error marks its span as failed.

use http::HeaderMap;
use tracing::Span;
use ulo::TraceContext;
use ulo_transport::{ErrorKind, span};

use crate::gateway::ConnectRefused;

/// The header a context is read from; a handshake without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";
const TRACESTATE: &str = "tracestate";

/// The context the handshake's `traceparent` and `tracestate` fields carry, every field under
/// each name, through `TraceContext::extract`; `None` when there is none to continue, which never
/// refuses the upgrade (H4).
///
/// A `traceparent` field that is not visible ASCII is off the grammar, so the connection carries
/// no context. A `tracestate` field that is not visible ASCII discards the whole list, as any
/// member off the grammar does, and leaves the `traceparent` standing.
pub(crate) fn extract(headers: &HeaderMap) -> Option<TraceContext> {
    let traceparent = texts(headers, TRACEPARENT)?;
    let tracestate = texts(headers, TRACESTATE).unwrap_or_default();
    TraceContext::extract(traceparent, tracestate)
}

/// Every value under `name`, or `None` when one of them is not text a header carries.
fn texts<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<Vec<&'a str>> {
    headers.get_all(name).iter().map(|value| value.to_str().ok()).collect()
}

/// Records `otel.status_code` as an error on a message's or a connect's span when it fails with
/// `kind`, a kind HTTP answers with a 5xx: the server's own failure, an event nothing handles
/// (`Unimplemented`) and a connection refused for capacity (`Unavailable`) included. A client's
/// mistake or a refusal it can act on, the kinds HTTP answers with a 4xx, a guard's `Forbidden`
/// among them, leaves the status unset.
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

/// [`record_failure`] for a refused connection: by its kind when it has one, and for a refusal
/// written as a close code alone, `ConnectRefused::code`, by the code: 1011 (internal error), 1012
/// (service restart), 1013 (try again later) and 1014 (bad gateway) are the server's conditions,
/// as RFC 6455 §7.4 and the IANA registry give them, and every other code the protocol's own
/// refusal of the client.
pub(crate) fn record_refusal(call_span: &Span, refused: &ConnectRefused) {
    match refused.kind {
        Some(kind) => record_failure(call_span, kind),
        None if matches!(refused.close_code, 1011..=1014) => {
            call_span.record(span::OTEL_STATUS_CODE, span::STATUS_ERROR);
        }
        None => {}
    }
}
