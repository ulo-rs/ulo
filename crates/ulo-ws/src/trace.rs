//! W3C Trace Context on a WebSocket connection (integrations §6.2, I9): extracted from the
//! handshake's headers into the connect phase's execution. The connection keeps the connect
//! execution's context and opens each later execution, a message's, a hand-written gateway's and
//! the terminal `on_disconnect`, with it as the inbound context, through
//! `Connection::exec_options`. The framework has no WebSocket client, so nothing is injected here.

use http::HeaderMap;
use ulo::TraceContext;

/// The header a context is read from; a handshake without it carries none.
pub(crate) const TRACEPARENT: &str = "traceparent";

/// The context the handshake's `traceparent` and `tracestate` fields carry, every field under
/// each name, through `TraceContext::extract`; `None` when there is none to continue, which never
/// refuses the upgrade (H4).
pub(crate) fn extract(headers: &HeaderMap) -> Option<TraceContext> {
    let _ = headers;
    todo!("O1: extraction from `traceparent` and `tracestate`")
}
