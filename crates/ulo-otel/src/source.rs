use tracing::Span;
use ulo::{TraceContext, TraceSource};

/// The `TraceSource` implementation. `start` converts the inbound context into an
/// `opentelemetry::Context`, sets it with `OpenTelemetrySpanExt::set_parent` before the span is first
/// entered, and reads `span.context()` back as the execution's own context, which `set_trace` marks
/// `Originated`. An absent or invalid inbound context starts a new trace.
pub(crate) struct OtelSource;

impl TraceSource for OtelSource {
    fn start(&self, span: &Span, inbound: Option<&TraceContext>) -> TraceContext {
        let _ = (span, inbound);
        todo!()
    }
}
