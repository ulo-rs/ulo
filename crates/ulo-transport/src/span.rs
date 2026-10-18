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
//! `otel.kind` is written by [`call`] from its [`Kind`], since a tracer fixes a span's kind when
//! the span starts and the source starts it inside `call`. `otel.status_code` is declared empty
//! for the transport to record [`STATUS_ERROR`] on a failure its rule counts as the server's, a
//! 5xx on HTTP and not a 4xx; a status recorded after the span started still reaches it.

use std::sync::Arc;

use tracing::Span;
use tracing::field::Empty;
use ulo::{AppHandle, Execution, TraceSource};

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
    );
    if let Some(source) = app.trace_source() {
        start(source, &span, exec);
    }
    span
}

fn start(source: &Arc<dyn TraceSource>, span: &Span, exec: &Execution) {
    let own = source.start(span, exec.trace());
    exec.set_trace(own);
}
