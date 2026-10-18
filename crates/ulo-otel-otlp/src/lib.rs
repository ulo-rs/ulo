//! OTLP export over HTTP with protobuf (integrations DESIGN §6.4), the exporter `ulo-otel` takes.
//!
//! ```ignore
//! let otel = ulo_otel::Otel::new(options, ulo_otel_otlp::OtlpExporter::http())?;
//! ```
//!
//! By default the HTTP client is the crate's own over `ureq`, blocking inside its async `send_bytes`
//! on the batch processor's thread, so an export reaches a receiver from a caller with no runtime
//! and the tree holds no tokio. The `reqwest-blocking` feature selects reqwest's blocking client
//! instead, which runs tokio on a thread of its own and never touches the app's.
//!
//! `ureq`'s rustls client is built with an explicit crypto provider, since rustls's own resolution
//! panics in a process that links two providers and installs neither. Where it cannot be built the
//! exporter refuses at construction: "this process links more than one rustls crypto provider and
//! installs none; install one with `CryptoProvider::install_default` before the app starts".

mod client;

use opentelemetry_otlp::SpanExporter;
use ulo::BoxError;
use ulo_otel::{Exporter, OtelOptions};

/// OTLP over HTTP with protobuf, the exporter's default protocol. Endpoint and headers come from
/// the [`OtelOptions`] it is built with.
#[derive(Clone, Debug, Default)]
pub struct OtlpExporter {
    _private: (),
}

impl OtlpExporter {
    pub fn http() -> Self {
        OtlpExporter { _private: () }
    }
}

impl Exporter for OtlpExporter {
    type Span = SpanExporter;

    fn build(self, options: &OtelOptions) -> Result<SpanExporter, BoxError> {
        let _ = options;
        todo!()
    }
}
