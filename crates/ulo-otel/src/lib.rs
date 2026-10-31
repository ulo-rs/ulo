//! OpenTelemetry export for the spans the transports already open (integrations DESIGN §6.4).
//!
//! ```
//! use tracing_subscriber::filter::LevelFilter;
//! use tracing_subscriber::layer::SubscriberExt;
//! use ulo_config::Env;
//! use ulo_otel::{Exporter, Otel, OtelOptions};
//!
//! // `exporter` is `ulo_otel_otlp::OtlpExporter::http()` against a collector.
//! fn install(exporter: impl Exporter) -> Result<Otel, ulo::BoxError> {
//!     let otel = Otel::new(OtelOptions::from_env(&Env::process())?, exporter)?;
//!     // h2 and hyper open trace-level spans: without the filter one request through the
//!     // example chain exported 170 spans.
//!     tracing::subscriber::set_global_default(tracing_subscriber::registry().with(LevelFilter::INFO).with(otel.layer()))?;
//!     Ok(otel)
//! }
//! // in the root module: m.import(otel.module());
//! ```
//!
//! `ulo_transport::span::call` opens a `tracing` span per call with OpenTelemetry's semantic
//! conventions; this crate exports those spans and adds none. The subscriber is the application's:
//! [`Otel::layer`] hands over a layer for it. The parent of a `tracing` span can be set only by code
//! holding the `tracing::Span`, after its creation and before it is first entered, so the crate
//! binds a `ulo::TraceSource` that `span::call` calls in that window. The source reads the incoming
//! context into an OpenTelemetry context, sets it as the span's parent, and reads the span's context
//! back as the execution's own, which the core marks `Originated`.
//!
//! Sampling is parent-based: a remote parent whose sampled flag is clear exports nothing. The batch
//! processor exports from a thread of its own; [`OtelModule`] flushes it in `OnApplicationShutdown`,
//! after the sockets close so the transports' last spans are included, on a thread of its own so no
//! worker blocks, under a bound. On expiry the thread is left running and the hook is recorded
//! `TimedOut`.
//!
//! # The process environment
//!
//! [`OtelOptions::from_env`] reads the `OTEL_*` names it lists through the `Env` the application
//! passes. Below it, the SDK reads `OTEL_BSP_*` and `OTEL_SPAN_*_LIMIT`, and the exporter crate
//! reads the `OTEL_EXPORTER_OTLP_*` names, straight from the process environment. Only variables
//! the OpenTelemetry specification defines are read that way by ulo's code and the crates it
//! calls for OpenTelemetry (the HTTP client's proxy variables are the one other read; see
//! `ulo-otel-otlp`). A value ulo passes explicitly (the service
//! name, the sampler, the endpoint, the timeout) wins over the environment's; request headers are
//! the exception, where the environment's entries are merged over the options' (see
//! `ulo-otel-otlp`).

mod exporter;
mod flush;
mod options;
mod otel;
mod sampler;
mod source;

pub use exporter::Exporter;
pub use options::OtelOptions;
pub use otel::{Otel, OtelModule};
