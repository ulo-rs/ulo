//! OpenTelemetry export for the spans the transports already open (integrations DESIGN §6.4).
//!
//! ```ignore
//! let otel = Otel::new(OtelOptions::from_env(&Env::process())?, OtlpExporter::http())?;
//! tracing_subscriber::registry().with(otel.layer()).init();
//! // in the root module:
//! m.import(otel.module());
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

mod exporter;
mod flush;
mod options;
mod otel;
mod sampler;
mod source;

pub use exporter::Exporter;
pub use options::OtelOptions;
pub use otel::{Otel, OtelModule};
