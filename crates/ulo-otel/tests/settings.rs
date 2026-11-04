//! `OtelModule`'s settings: its identity is its type, so two imports are one module, and the
//! exporter's `Exporter::settings` decide whether the second is the same one. The app runs on the
//! test's runtime because the module's flush hook is bounded, which needs a `Timer`.

use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use ulo::{App, BoxError, DifferingSetting, Module, ModuleDef, ModuleIdentity, ModuleSettings, Secret, StartupError, WiringError, WiringErrors};
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

/// Two exporters of one type are one module, whatever endpoint their options name: an opaque
/// exporter compares by type name alone.
#[tokio::test]
async fn two_exporters_of_one_type_are_one_module() {
    let endpoint = |url: &str| OtelOptions { endpoint: Some(url.to_owned()), ..OtelOptions::default() };
    App::builder(Root(vec![otel(endpoint("http://a:4318"), Opaque), otel(endpoint("http://b:4318"), Opaque)]))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("one exporter type is one module:\n{error}"));
}

/// The options the module reads itself are settings whatever the exporter: another
/// `sample_ratio` is refused naming it. Planted break: `Otel::new` keeping the exporter's
/// settings alone.
#[tokio::test]
async fn two_sample_ratios_under_one_identity_are_refused() {
    let ratio = |r: f64| OtelOptions { sample_ratio: Some(r), ..OtelOptions::default() };
    let errors = refused(Root(vec![otel(ratio(0.5), Opaque), otel(ratio(1.0), Opaque)]));
    assert_eq!(differing(&errors), [("sample_ratio", Some("0.5".to_owned()), Some("1.0".to_owned()))]);
}

/// The ratio and the flush timeout compare as the module runs them: unset is the documented
/// default, and `-0.0` is `0.0`. An unset `service_name` differs from one written out. Planted
/// breaks: the ratio compared as written; `-0.0` kept.
#[tokio::test]
async fn a_ratio_and_a_flush_timeout_compare_as_the_module_runs_them() {
    let wired = |a: OtelOptions, b: OtelOptions| {
        App::builder(Root(vec![otel(a, Opaque), otel(b, Opaque)]))
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .map(drop)
            .unwrap_or_else(|error| panic!("equal as run, one module:\n{error}"));
    };
    let ratio = |r: Option<f64>| OtelOptions { sample_ratio: r, ..OtelOptions::default() };
    wired(ratio(None), ratio(Some(1.0)));
    wired(ratio(Some(0.0)), ratio(Some(-0.0)));
    let flush = |ms: Option<u64>| OtelOptions { flush_timeout_ms: ms, ..OtelOptions::default() };
    wired(flush(None), flush(Some(5000)));

    let errors = refused(Root(vec![otel(ratio(Some(0.0)), Opaque), otel(ratio(None), Opaque)]));
    assert_eq!(differing(&errors), [("sample_ratio", Some("0.0".to_owned()), Some("1.0".to_owned()))]);
    let name = |n: Option<&str>| OtelOptions { service_name: n.map(str::to_owned), ..OtelOptions::default() };
    let errors = refused(Root(vec![otel(name(None), Opaque), otel(name(Some("checkout")), Opaque)]));
    assert_eq!(differing(&errors), [("service_name", Some("None".to_owned()), Some("Some(\"checkout\")".to_owned()))]);
}

/// An exporter writing a name the module writes too.
struct Colliding;

impl Exporter for Colliding {
    type Span = Sink;

    fn settings(&self, _options: &OtelOptions) -> ModuleSettings {
        ModuleSettings::new().with("service_name", "from the exporter")
    }

    fn build(self, _options: &OtelOptions) -> Result<Sink, BoxError> {
        Ok(Sink)
    }
}

/// An exporter's setting under a name the module also writes fails `Otel::new`, rather than one
/// of the two values going uncompared. Planted break: `ModuleSettings::with` accepting a name
/// twice.
#[test]
#[should_panic(expected = "the module setting `service_name` is written twice")]
fn an_exporter_setting_named_as_a_module_setting_panics() {
    let _ = Otel::new(OtelOptions::default(), Colliding);
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
