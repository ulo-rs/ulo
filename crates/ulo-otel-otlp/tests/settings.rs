//! `OtlpExporter`'s settings, which `OtelModule` compares: `otlp-http`, the protocol and the
//! endpoint without its userinfo, never the headers.

use ulo::{App, DifferingSetting, Module, ModuleDef, ModuleIdentity, ModuleSettings, Secret, StartupError, WiringError, WiringErrors};
use ulo_otel::{Exporter, Otel, OtelOptions};
use ulo_otel_otlp::OtlpExporter;

/// The headers each registration's options carry, one per side of a refusal.
const HEADERS: [&str; 2] = ["first-header-credential", "second-header-credential"];

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

fn options(endpoint: &str, header: &str) -> OtelOptions {
    OtelOptions {
        endpoint: Some(endpoint.to_owned()),
        headers: Some(Secret::new(format!("authorization={header}"))),
        ..OtelOptions::default()
    }
}

fn otel(endpoint: &str, header: &str) -> Otel {
    Otel::new(options(endpoint, header), OtlpExporter::http()).unwrap_or_else(|error| panic!("the exporter builds: {error}"))
}

fn holds_a_header(text: &str) -> bool {
    HEADERS.iter().any(|header| text.contains(header))
}

fn wired(root: Root) -> Result<(), WiringErrors> {
    match App::builder(root).runtime(ulo_runtime_tokio::Tokio::current()).wire() {
        Ok(_) => Ok(()),
        Err(StartupError::Wiring(errors)) => Err(errors),
        Err(other) => panic!("expected wiring errors, got: {other}"),
    }
}

fn sides(setting: &DifferingSetting) -> (&'static str, Option<String>, Option<String>) {
    (setting.name, setting.first.clone(), setting.second.clone())
}

/// Planted break: `OtlpExporter::settings` left at the trait's default, the type name alone.
#[test]
fn the_settings_name_the_kind_the_endpoint_and_the_protocol() {
    let settings = OtlpExporter::http().settings(&options("https://user:key@collector:4318/base", HEADERS[0]));
    let expected = ModuleSettings::new()
        .with("exporter", format_args!("otlp-http"))
        .with("endpoint", format_args!("https://collector:4318/base"))
        .with("protocol", format_args!("http/protobuf"));
    assert_eq!(settings, expected);
    assert!(!holds_a_header(&format!("{settings:?}")), "{settings:?}");

    // Only the authority's userinfo is left out: an `@` in the path is kept.
    let at_in_path = OtlpExporter::http().settings(&options("https://collector:4318/team@example/v1", HEADERS[0]));
    assert!(format!("{at_in_path:?}").contains("https://collector:4318/team@example/v1"), "{at_in_path:?}");
}

/// Two endpoints under one identity are refused, naming the endpoint, and the headers, which
/// differ too, appear nowhere in the refusal. Planted breaks: the endpoint left out of the
/// settings; the headers written into them.
#[tokio::test]
async fn two_endpoints_under_one_identity_are_refused_without_the_headers() {
    let errors = match wired(Root(vec![otel("http://a:4318", HEADERS[0]), otel("http://b:4318", HEADERS[1])])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two endpoints under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(sides).collect();
    assert_eq!(settings, [("endpoint", Some("http://a:4318".to_owned()), Some("http://b:4318".to_owned()))]);
    let text = errors.to_string();
    assert!(!holds_a_header(&text), "{text}");
}

/// Equal endpoints are one module, and so are two that differ only in their userinfo. Planted
/// break: the endpoint compared with its userinfo.
#[tokio::test]
async fn one_endpoint_is_one_module_whatever_its_userinfo() {
    let header = HEADERS[0];
    wired(Root(vec![otel("http://a:4318", header), otel("http://a:4318", header)])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec![otel("http://one:x@a:4318", header), otel("http://two:y@a:4318", header)]))
        .unwrap_or_else(|errors| panic!("{errors}"));
}
