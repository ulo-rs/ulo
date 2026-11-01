use opentelemetry_sdk::trace::SpanExporter;
use ulo::BoxError;

use crate::options::OtelOptions;

/// The slot `ulo-otel-otlp`'s `OtlpExporter` fills: how spans leave the process.
///
/// A trait with an associated exporter type and not a boxed `SpanExporter`, since the SDK's trait
/// returns `impl Future` and so is not dyn-compatible. `build` may refuse, and `Otel::new` returns
/// the refusal.
pub trait Exporter {
    type Span: SpanExporter + 'static;

    fn build(self, options: &OtelOptions) -> Result<Self::Span, BoxError>;
}
