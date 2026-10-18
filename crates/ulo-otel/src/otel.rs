use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity};

use crate::exporter::Exporter;
use crate::options::OtelOptions;

/// The tracer provider, built in `main` before the application's subscriber is installed.
#[derive(Clone)]
pub struct Otel {
    pub(crate) provider: SdkTracerProvider,
    pub(crate) options: OtelOptions,
}

impl Otel {
    /// Builds the provider with a batch processor over the exporter `exporter` builds, and the
    /// sampler the options name. Fails when the exporter refuses its construction.
    pub fn new<E: Exporter>(options: OtelOptions, exporter: E) -> Result<Otel, BoxError> {
        let _ = (options, exporter);
        todo!()
    }

    /// The layer for the application's `tracing` subscriber.
    pub fn layer<S>(&self) -> OpenTelemetryLayer<S, SdkTracer>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        todo!()
    }

    /// The module to import: it binds the `TraceSource`, which the app resolves once at `connect`,
    /// and flushes the batch processor in `OnApplicationShutdown`.
    pub fn module(&self) -> OtelModule {
        OtelModule { otel: self.clone() }
    }
}

/// Binds the source and the flush; built by [`Otel::module`].
pub struct OtelModule {
    pub(crate) otel: Otel,
}

impl Module for OtelModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = (m, &self.otel);
        todo!()
    }
}
