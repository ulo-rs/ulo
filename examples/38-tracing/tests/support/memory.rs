//! An exporter that keeps the spans in memory, for a test to read: `ulo-otel` takes an
//! `Exporter`, and the SDK's `SpanExporter` is what it builds.

use std::sync::{Arc, Mutex, PoisonError};

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use ulo::BoxError;
use ulo_otel::{Exporter, OtelOptions};

#[derive(Clone, Default)]
pub struct Collected(Arc<Mutex<Vec<SpanData>>>);

#[derive(Debug)]
pub struct Sink(Arc<Mutex<Vec<SpanData>>>);

impl SpanExporter for Sink {
    fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).extend(batch);
        async { Ok(()) }
    }
}

impl Exporter for Collected {
    type Span = Sink;

    fn build(self, _options: &OtelOptions) -> Result<Sink, BoxError> {
        Ok(Sink(self.0))
    }
}

impl Collected {
    pub fn spans(&self) -> Vec<SpanData> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
