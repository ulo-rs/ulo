use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use futures_channel::oneshot;
use opentelemetry::InstrumentationScope;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::resource::TelemetryResourceDetector;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use tracing::Subscriber;
use tracing_opentelemetry::OpenTelemetryLayer;
use tracing_subscriber::registry::LookupSpan;
use ulo::{BoxError, Module, ModuleDef, ModuleIdentity, ModuleSettings, Secret, Signal, TraceSource};

use crate::exporter::Exporter;
use crate::flush;
use crate::options::OtelOptions;
use crate::sampler::sampler;
use crate::source::OtelSource;

/// The instrumentation scope the layer's tracer reports.
const SCOPE: &str = "ulo-otel";

/// What the shutdown hook's bound adds to the flush timeout: the flush thread reports after the
/// processor's own timeout, so a bound equal to it would expire on a flush that is about to report.
const HOOK_MARGIN: Duration = Duration::from_secs(1);

/// The tracer provider, built in `main` before the application's subscriber is installed.
#[derive(Clone)]
pub struct Otel {
    pub(crate) provider: SdkTracerProvider,
    pub(crate) options: OtelOptions,
    /// The exporter's `Exporter::settings`, read before `build` consumed it, and the options the
    /// module itself reads.
    pub(crate) settings: ModuleSettings,
    /// Shared with the source: set once the flush begins.
    pub(crate) flushing: Arc<AtomicBool>,
}

impl Otel {
    /// Builds the provider with a batch processor over the exporter `exporter` builds, and the
    /// sampler the options name. Fails when the exporter refuses its construction, and on options
    /// no reader can mean: a ratio that is not a number from 0 to 1, a zero flush timeout, or
    /// headers off the `key=value` grammar.
    ///
    /// The batch processor starts its export thread here. The resource carries `service.name`
    /// and the SDK's `telemetry.sdk.*` attributes alone; it reads no `OTEL_*` variable, so a
    /// value comes from the options however they were read. The SDK's own defaults still read
    /// `OTEL_BSP_*` and the `OTEL_SPAN_*_LIMIT` names from the process environment.
    ///
    /// # Panics
    ///
    /// When the exporter's [`Exporter::settings`] write `service_name`, `sample_ratio` or
    /// `flush_timeout_ms`, the names the module writes itself.
    #[track_caller]
    pub fn new<E: Exporter>(options: OtelOptions, exporter: E) -> Result<Otel, BoxError> {
        options.check()?;
        // The ratio and the flush timeout are compared as the module runs them, an unset one as
        // its documented default. `Debug` writes an `f64` as the shortest text reading back to its
        // bits, and `check` has refused NaN; adding `0.0` turns `-0.0` into `0.0`, the one ratio
        // they both sample at.
        let settings = exporter
            .settings(&options)
            .with("service_name", &options.service_name)
            .with("sample_ratio", options.sample_ratio.unwrap_or(1.0) + 0.0)
            .with("flush_timeout_ms", options.flush_timeout().as_millis());
        let spans = exporter.build(&options)?;
        let provider = SdkTracerProvider::builder()
            .with_resource(resource(&options))
            .with_sampler(sampler(&options))
            .with_batch_exporter(spans)
            .build();
        Ok(Otel { provider, options, settings, flushing: Arc::new(AtomicBool::new(false)) })
    }

    /// The layer for the application's `tracing` subscriber.
    ///
    /// It exports every span the subscriber passes it, the transports' and the application's
    /// own; a span the subscriber filters out is not exported and gets no OpenTelemetry IDs.
    /// Put a level filter ahead of it: h2 and hyper open trace-level spans, and a request through
    /// the HTTP server exports a hundred and more of them under an unfiltered subscriber. The
    /// crate documentation shows the subscriber with `LevelFilter::INFO`.
    pub fn layer<S>(&self) -> OpenTelemetryLayer<S, SdkTracer>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        let scope = InstrumentationScope::builder(SCOPE).with_version(env!("CARGO_PKG_VERSION")).build();
        OpenTelemetryLayer::new(self.provider.tracer_with_scope(scope))
    }

    /// The module to import: it binds the `TraceSource`, which the app resolves once at `connect`,
    /// and flushes the batch processor in `OnApplicationShutdown`.
    pub fn module(&self) -> OtelModule {
        OtelModule { otel: self.clone() }
    }
}

/// Binds the source and the flush; built by [`Otel::module`].
///
/// The module is global, so the app finds its source from the root module wherever it is
/// imported. Its flush hook is bounded a second past the flush timeout, which needs the app's
/// `Timer`, as every bound written on a hook does; past the bound the hook is recorded `TimedOut`
/// and the flush thread is left to finish on its own. A span another `OnApplicationShutdown` hook
/// opens after the flush is not exported.
///
/// Its identity is its type, so two imports are one module, and its settings are the exporter's
/// [`Exporter::settings`] followed by the options' `service_name`, `sample_ratio` and
/// `flush_timeout_ms`: a second import over an exporter answering other settings, or with another
/// of those options, is refused at `wire()` as `ulo::WiringError::DifferingSettings`. The ratio
/// and the flush timeout compare as the module runs them: an unset ratio is `1.0` and an unset
/// timeout `5000`, so either written out equals it unset, and `-0.0` is the ratio `0.0`. An unset
/// `service_name` is not compared as the name the resource derives, which depends on the
/// executable, so it differs from every name written out. The headers are never among them.
pub struct OtelModule {
    pub(crate) otel: Otel,
}

impl Module for OtelModule {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn settings(&self) -> ModuleSettings {
        self.otel.settings.clone()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.global();
        let options = &self.otel.options;
        if let Some(headers) = &options.headers
            && !headers.expose().is_empty()
        {
            m.secret(headers);
        }
        for value in options.header_values() {
            m.secret(&Secret::new(value));
        }

        m.value(OtelSource { flushing: Arc::clone(&self.otel.flushing) }).also_as::<dyn TraceSource>(|source| source);
        m.export::<dyn TraceSource>();

        let provider = self.otel.provider.clone();
        let flushing = Arc::clone(&self.otel.flushing);
        let timeout = options.flush_timeout();
        m.on_shutdown(move |_signal: Signal| {
            flushing.store(true, Ordering::Release);
            let finished = flush::spawn(provider.clone(), timeout);
            async move {
                match finished.await {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        tracing::warn!(%error, "the span flush at shutdown failed; spans still queued are lost");
                    }
                    Err(oneshot::Canceled) => {
                        tracing::warn!("the span flush thread ended without reporting; spans still queued may be lost");
                    }
                }
            }
        })
        .timeout(timeout.saturating_add(HOOK_MARGIN));
    }
}

fn resource(options: &OtelOptions) -> Resource {
    let service = match &options.service_name {
        Some(name) => name.clone(),
        None => unknown_service(),
    };
    Resource::builder_empty()
        .with_detector(Box::new(TelemetryResourceDetector))
        .with_service_name(service)
        .build()
}

/// `unknown_service:` and the executable's name, the semantic conventions' value for a service
/// that names none, or `unknown_service` alone when the name cannot be read.
fn unknown_service() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .map_or_else(|| String::from("unknown_service"), |name| format!("unknown_service:{name}"))
}
