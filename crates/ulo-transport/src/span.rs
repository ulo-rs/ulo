//! The tracing span each transport wraps `dispatch` in, named and attributed by the
//! OpenTelemetry semantic conventions (transports DESIGN §2.9), and parented on the call's trace
//! context through the app's `TraceSource` (integrations §6.4, I7).
//!
//! A `tracing` span declares its fields where it is created, so [`call`] declares every field any
//! transport records, empty, and each transport records its own: HTTP `http.request.method`,
//! `http.route`, `url.path`, `url.scheme`, `http.response.status_code`; RPC `rpc.system` (`"ulo"`),
//! `rpc.method` and, on a broker, `messaging.system`; gRPC `rpc.system = "grpc"`, `rpc.service`,
//! `rpc.method`, `rpc.grpc.status_code`; WebSocket, which has no convention of its own, `ws.event`
//! on a message's span and `url.path` on the connect phase's. The span's display name,
//! `GET /users/{id}`, `users.v1.UserService/GetUser` or the event `chat.send`, is the `otel.name`
//! field. Every span carries `ulo.transport`
//! (the transport's key) and `ulo.handler`.
//!
//! Every span also records `trace_id`, the name the OpenTelemetry log data model and JSON log
//! pipelines join on, so a log line written inside a call carries what correlates it with the
//! call's other lines, with an exporter installed or without one. The value is 32 lowercase hex digits, a W3C trace ID,
//! never all zeros: the execution's context's trace ID when it has one, the `TraceSource`'s
//! answer with a source bound and the caller's `traceparent` in pass-through, and otherwise one
//! generated for the call. A generated ID is written on the span alone. It is not the
//! execution's context, so a client called `.within` the execution carries no `traceparent` for
//! it and a downstream service does not share it; correlation across services needs a caller's
//! context or a bound `TraceSource`.
//!
//! `otel.kind` is written by [`call`] from its [`Kind`], since a tracer fixes a span's kind when
//! the span starts and the source starts it inside `call`. `otel.status_code` is declared empty
//! for the transport to record [`STATUS_ERROR`] on a failure its rule counts as the server's, a
//! 5xx on HTTP and not a 4xx; a status recorded after the span started still reaches it.

use std::sync::Arc;

use tracing::Span;
use tracing::field::Empty;
use ulo::{AppHandle, Execution, TraceContext, TraceSource};

pub const OTEL_NAME: &str = "otel.name";
pub const OTEL_KIND: &str = "otel.kind";
pub const OTEL_STATUS_CODE: &str = "otel.status_code";
pub const TRANSPORT: &str = "ulo.transport";
pub const HANDLER: &str = "ulo.handler";
pub const HTTP_REQUEST_METHOD: &str = "http.request.method";
pub const HTTP_ROUTE: &str = "http.route";
pub const URL_PATH: &str = "url.path";
pub const URL_SCHEME: &str = "url.scheme";
pub const HTTP_RESPONSE_STATUS_CODE: &str = "http.response.status_code";
pub const RPC_SYSTEM: &str = "rpc.system";
pub const RPC_SERVICE: &str = "rpc.service";
pub const RPC_METHOD: &str = "rpc.method";
pub const RPC_GRPC_STATUS_CODE: &str = "rpc.grpc.status_code";
pub const MESSAGING_SYSTEM: &str = "messaging.system";
pub const WS_EVENT: &str = "ws.event";
pub const TRACE_ID: &str = "trace_id";

/// `otel.status_code`'s value for a call that failed by the transport's rule.
pub const STATUS_ERROR: &str = "error";
/// `otel.status_code`'s value for a call a transport marks as succeeded; OpenTelemetry leaves a
/// server span's status unset otherwise.
pub const STATUS_OK: &str = "ok";

/// The span kind `otel.kind` records, OpenTelemetry's five.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A call served: an HTTP request, a gRPC or RPC request, a WebSocket connect or message.
    Server,
    /// A message received with no reply expected, an RPC event's handler, a consumer of the
    /// producer's span in OpenTelemetry's messaging conventions.
    Consumer,
    Client,
    Producer,
    Internal,
}

impl Kind {
    /// The text `tracing-opentelemetry` reads from `otel.kind`.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Server => "server",
            Kind::Consumer => "consumer",
            Kind::Client => "client",
            Kind::Producer => "producer",
            Kind::Internal => "internal",
        }
    }
}

/// The span for one call of `exec`: `name` as `otel.name`, `kind` as `otel.kind`, `transport` as
/// `ulo.transport`, `handler` as `ulo.handler` when a handler matched, every other field above
/// declared empty for the transport to record.
///
/// With a `TraceSource` bound, `app.trace_source()`, the source starts the span on the execution's
/// context, the one the transport passed in `ExecOptions::trace`, before `call` returns it, and
/// its answer, the execution's own context, is written through `Execution::set_trace`. The span is
/// not entered here: the transport enters it, through `.instrument(..)`, after `call` returns,
/// since a tracer can set the parent only on a span not yet entered. With no source, nothing is
/// called and the execution keeps the context it was opened with.
///
/// `trace_id` is then recorded from the execution's context, or generated when it has none, as
/// the module documentation states.
pub fn call(app: &AppHandle, exec: &Execution, kind: Kind, transport: &'static str, name: &str, handler: Option<&str>) -> Span {
    // Each field name spells the constant of the same text above; `span.record` finds a field by
    // that text, and the macro takes names as written, not as constants.
    let span = tracing::info_span!(
        "call",
        otel.name = name,
        otel.kind = kind.as_str(),
        otel.status_code = Empty,
        ulo.transport = transport,
        ulo.handler = handler,
        http.request.method = Empty,
        http.route = Empty,
        url.path = Empty,
        url.scheme = Empty,
        http.response.status_code = Empty,
        rpc.system = Empty,
        rpc.service = Empty,
        rpc.method = Empty,
        rpc.grpc.status_code = Empty,
        messaging.system = Empty,
        ws.event = Empty,
        trace_id = Empty,
    );
    if let Some(source) = app.trace_source() {
        start(source, &span, exec);
    }
    // A span no subscriber records takes no ID: generating one would cost every call of an app
    // that installs none.
    if !span.is_disabled() {
        let trace_id = exec.trace().map_or_else(|| nonzero(draw), TraceContext::trace_id);
        span.record(TRACE_ID, hex(&trace_id).as_str());
    }
    span
}

fn start(source: &Arc<dyn TraceSource>, span: &Span, exec: &Execution) {
    let own = source.start(span, exec.trace());
    exec.set_trace(own);
}

/// A trace ID from `draw`, drawn again while it answers all zeros, which W3C Trace Context makes
/// invalid.
fn nonzero(mut draw: impl FnMut() -> u128) -> [u8; 16] {
    loop {
        let id = draw();
        if id != 0 {
            return id.to_be_bytes();
        }
    }
}

/// 128 bits from the standard library's `RandomState`: SipHash-1-3 under keys seeded from the
/// operating system's randomness once per thread and stepped for each new state, over a process
/// counter, so no two draws hash the same input under the same keys. Not cryptographic, and a
/// trace ID needs no more: it is unique with overwhelming probability, carries no secret and
/// leaves the process only on a span.
fn draw() -> u128 {
    use std::hash::{BuildHasher, RandomState};
    use std::sync::atomic::{AtomicU64, Ordering};

    static DRAWS: AtomicU64 = AtomicU64::new(0);
    let n = DRAWS.fetch_add(1, Ordering::Relaxed);
    let keys = RandomState::new();
    (u128::from(keys.hash_one((n, 0u8))) << 64) | u128::from(keys.hash_one((n, 1u8)))
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{draw, hex, nonzero};

    #[test]
    fn an_all_zero_draw_is_drawn_again() {
        let mut draws = [0u128, 0, 0x2a].into_iter();
        let id = nonzero(|| draws.next().expect("a draw after the zeros"));
        assert_eq!(id, 0x2au128.to_be_bytes());
        assert_eq!(draws.next(), None, "a draw after the first nonzero one");
    }

    #[test]
    fn a_trace_id_is_written_as_32_lowercase_hex_digits_most_significant_first() {
        let id = nonzero(|| 0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);
        assert_eq!(hex(&id), "0123456789abcdeffedcba9876543210");
        assert_eq!(hex(&nonzero(|| 1)), "00000000000000000000000000000001");
    }

    #[test]
    fn successive_draws_differ() {
        let ids: std::collections::HashSet<u128> = (0..1_000).map(|_| draw()).collect();
        assert_eq!(ids.len(), 1_000, "two of a thousand draws were equal");
    }
}
