//! OTLP export over HTTP with protobuf (integrations DESIGN §6.4), the exporter `ulo-otel` takes.
//!
//! ```
//! # use ulo_otel::OtelOptions;
//! # fn f(options: OtelOptions) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
//! let otel = ulo_otel::Otel::new(options, ulo_otel_otlp::OtlpExporter::http())?;
//! # Ok(())
//! # }
//! ```
//!
//! The HTTP client is the crate's own over `ureq` (feature `ureq`, on by default), blocking inside
//! its async `send_bytes` on the batch processor's thread, so an export reaches a receiver from a
//! caller with no runtime and the tree holds no tokio. The `reqwest-blocking` feature adds
//! reqwest's blocking client, which runs tokio on a thread of its own and never touches the app's.
//! The features are additive. An application that wants reqwest alone disables default features
//! and enables `reqwest-blocking`; with both, [`OtlpExporter::http`] uses `ureq` and
//! [`OtlpExporter::http_over`] names the client. At least one of the two is required.
//!
//! `ureq`'s rustls client is built with an explicit crypto provider (`ring`, the one the workspace's
//! rustls is built with) and never asks rustls to resolve one, which panics in a process that
//! links two providers and installs neither. Roots come from the platform's verifier. The
//! `reqwest-blocking` client is handed the same: rustls over `ring` with the platform's verifier,
//! and an `https` endpoint reached through a proxy is tunnelled with `CONNECT`.
//!
//! The options' `endpoint` is the receiver's base URL, as `OTEL_EXPORTER_OTLP_ENDPOINT` is, and
//! `/v1/traces` is appended to it. With no endpoint the exporter reads the standard `OTEL_*`
//! variables itself and otherwise sends to `http://localhost:4318/v1/traces`. A request that has
//! not finished in ten seconds fails.
//!
//! # The process environment
//!
//! Only variables the OpenTelemetry specification defines are read from the process environment,
//! and they are read by the exporter crate and the SDK beneath this one, not through the `Env`
//! the application passes to [`OtelOptions::from_env`](ulo_otel::OtelOptions::from_env). Where
//! ulo passes a value explicitly, that value wins over the environment's: the endpoint (the
//! per-signal `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` included), the request timeout, the protocol
//! and the headers. A header the options name replaces one of the same name from
//! `OTEL_EXPORTER_OTLP_HEADERS` or `OTEL_EXPORTER_OTLP_TRACES_HEADERS`, because each client, the
//! `ureq` one and reqwest's, applies the options' headers
//! after the exporter has set the environment's. The batch processor's `OTEL_BSP_*` and the
//! `OTEL_SPAN_*_LIMIT` names are read by the SDK; the options carry no value for them.
//!
//! One read is not an OpenTelemetry variable: the client takes its proxy from the environment
//! (`ALL_PROXY`, `HTTPS_PROXY`, `HTTP_PROXY`, `NO_PROXY`), as ureq's and reqwest's defaults do, and
//! nothing else: reqwest is built without its `system-proxy` feature, so the macOS and Windows
//! system proxy settings are not read by either client. [`OtlpExporter::proxy`] names one
//! explicitly and [`OtlpExporter::no_proxy`] turns proxies off, each whatever the environment says,
//! with either client.

#[cfg(not(any(feature = "ureq", feature = "reqwest-blocking")))]
compile_error!("`ulo-otel-otlp` needs an HTTP client: enable the `ureq` feature (on by default) or `reqwest-blocking`");

// With neither client feature nothing below the `compile_error!` is compiled, so the one error a
// user sees is the one naming the two features.
#[cfg(feature = "ureq")]
mod client;
#[cfg(any(feature = "ureq", feature = "reqwest-blocking"))]
mod exporter;
#[cfg(any(feature = "ureq", feature = "reqwest-blocking"))]
mod proxy;
#[cfg(feature = "reqwest-blocking")]
mod reqwest_client;

#[cfg(any(feature = "ureq", feature = "reqwest-blocking"))]
pub use exporter::{Client, OtlpExporter};
