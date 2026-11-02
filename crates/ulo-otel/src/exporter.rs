use std::any::type_name;

use opentelemetry_sdk::trace::SpanExporter;
use ulo::{BoxError, ModuleSettings};

use crate::options::OtelOptions;

/// The slot `ulo-otel-otlp`'s `OtlpExporter` fills: how spans leave the process.
///
/// A trait with an associated exporter type and not a boxed `SpanExporter`, since the SDK's trait
/// returns `impl Future` and so is not dyn-compatible. `build` may refuse, and `Otel::new` returns
/// the refusal.
pub trait Exporter {
    type Span: SpanExporter + 'static;

    /// What [`OtelModule`](crate::OtelModule) answers from `Module::settings`, read before
    /// `build`: two registrations of the module under one identity whose exporters answer other
    /// settings are refused at `wire()` as `ulo::WiringError::DifferingSettings`.
    ///
    /// By default the exporter's type name alone, under `exporter`, so an exporter that overrides
    /// nothing compares by its type and two values of one type are one module. An exporter that
    /// can tell its configurations apart writes its kind under `exporter`, and `endpoint` and
    /// `protocol` where it has them, as `ulo-otel-otlp`'s `otlp-http` does. A wiring report prints
    /// the values, so a header or any other credential is never written here.
    fn settings(&self, _options: &OtelOptions) -> ModuleSettings {
        ModuleSettings::new().with("exporter", format_args!("{}", type_name::<Self>()))
    }

    fn build(self, options: &OtelOptions) -> Result<Self::Span, BoxError>;
}
