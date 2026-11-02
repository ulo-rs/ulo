//! `OtelModule`'s settings: its identity is its type, so two imports are one module, and the
//! exporter's `Exporter::settings` decide whether the second is the same one. The app runs on the
//! test's runtime because the module's flush hook is bounded, which needs a `Timer`.

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use ulo::{App, BoxError, DifferingSetting, Module, ModuleDef, ModuleIdentity, Secret, StartupError, WiringError, WiringErrors};
use ulo_otel::{Exporter, Otel, OtelOptions};

#[derive(Debug)]
struct Sink;

impl SpanExporter for Sink {
    async fn export(&self, _batch: Vec<SpanData>) -> OTelSdkResult {
        Ok(())
    }
}

/// An exporter that answers the default settings, its type name.
struct Opaque;

impl Exporter for Opaque {
    type Span = Sink;

    fn build(self, _options: &OtelOptions) -> Result<Sink, BoxError> {
        Ok(Sink)
    }
}

/// A second exporter type, also on the default settings.
struct OtherOpaque;

impl Exporter for OtherOpaque {
    type Span = Sink;

    fn build(self, _options: &OtelOptions) -> Result<Sink, BoxError> {
        Ok(Sink)
    }
}

/// Imports `otel.module()` for each, in order.
struct Root(Vec<Otel>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Root>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for otel in &self.0 {
            m.import(otel.module());
        }
    }
}

fn otel(options: OtelOptions, exporter: impl Exporter) -> Otel {
    Otel::new(options, exporter).unwrap_or_else(|error| panic!("the exporter builds: {error}"))
}

fn refused(root: Root) -> WiringErrors {
    match App::builder(root).runtime(ulo_runtime_tokio::Tokio::current()).wire() {
        Err(StartupError::Wiring(errors)) => errors,
        Err(other) => panic!("expected wiring errors, got: {other}"),
        Ok(_) => panic!("wired; two exporters under one identity should be refused"),
    }
}

/// The one `DifferingSettings` entry, each setting as `(name, first, second)`.
fn differing(errors: &WiringErrors) -> Vec<(&'static str, Option<String>, Option<String>)> {
    assert_eq!(errors.len(), 1, "{errors}");
    match errors.iter().next() {
        Some(WiringError::DifferingSettings { settings, .. }) => settings.iter().map(sides).collect(),
        other => panic!("expected `DifferingSettings`, got {other:?}"),
    }
}

fn sides(setting: &DifferingSetting) -> (&'static str, Option<String>, Option<String>) {
    (setting.name, setting.first.clone(), setting.second.clone())
}

/// A user's exporter compares by its type name. Planted breaks: `OtelModule::settings` left at
/// the trait's default; `Exporter::settings` answering no setting.
#[tokio::test]
async fn two_exporter_types_under_one_identity_are_refused_by_type_name() {
    let errors = refused(Root(vec![otel(OtelOptions::default(), Opaque), otel(OtelOptions::default(), OtherOpaque)]));
    assert_eq!(
        differing(&errors),
        [("exporter", Some(std::any::type_name::<Opaque>().to_owned()), Some(std::any::type_name::<OtherOpaque>().to_owned()))]
    );
}

/// Two exporters of one type are one module, whatever their options say: an opaque exporter
/// compares by type name alone.
#[tokio::test]
async fn two_exporters_of_one_type_are_one_module() {
    let endpoint = |url: &str| OtelOptions { endpoint: Some(url.to_owned()), ..OtelOptions::default() };
    App::builder(Root(vec![otel(endpoint("http://a:4318"), Opaque), otel(endpoint("http://b:4318"), Opaque)]))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("one exporter type is one module:\n{error}"));
}

/// The headers are never a setting, in the entry's fields or in its text, though the two
/// registrations' headers differ. Planted break: the default `Exporter::settings` writing the
/// options' headers.
#[tokio::test]
async fn the_headers_never_appear_in_the_refusal() {
    const KEYS: [&str; 2] = ["first-header-credential", "second-header-credential"];
    let options = |key: &str| OtelOptions { headers: Some(Secret::new(format!("authorization={key}"))), ..OtelOptions::default() };
    let errors = refused(Root(vec![otel(options(KEYS[0]), Opaque), otel(options(KEYS[1]), OtherOpaque)]));
    let text = errors.to_string();
    for (name, first, second) in differing(&errors) {
        for value in [first, second].into_iter().flatten() {
            assert!(KEYS.iter().all(|key| !value.contains(key)), "`{name}` holds a header: {value}");
        }
    }
    assert!(KEYS.iter().all(|key| !text.contains(key)), "{text}");
}
