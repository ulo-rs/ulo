//! `OtlpExporter`'s settings, which `OtelModule` compares: `otlp-http`, the protocol, the client,
//! the proxy and the endpoint's scheme, host, port and path, a URL's userinfo, query and fragment
//! never among them, nor the headers.

use ulo::{App, DifferingSetting, Module, ModuleDef, ModuleIdentity, ModuleSettings, Secret, StartupError, WiringError, WiringErrors};
use ulo_otel::{Exporter, Otel, OtelOptions};
use ulo_otel_otlp::{Client, OtlpExporter};

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

/// The passwords an endpoint carries in its userinfo and in its query.
const PASSWORDS: [&str; 2] = ["userinfo-credential-value", "query-credential-value"];

fn holds_a_password(text: &str) -> bool {
    PASSWORDS.iter().any(|password| text.contains(password))
}

/// Planted breaks: `OtlpExporter::settings` left at the trait's default, the type name alone;
/// the endpoint written with its query.
#[test]
fn the_settings_name_the_kind_the_endpoint_and_the_protocol() {
    let endpoint = "https://user:userinfo-credential-value@collector:4318/base?api_key=query-credential-value#f";
    let settings = OtlpExporter::http().settings(&options(endpoint, HEADERS[0]));
    let expected = ModuleSettings::new()
        .with("exporter", format_args!("otlp-http"))
        .with("endpoint", format_args!("https://collector:4318/base"))
        .with("protocol", format_args!("http/protobuf"))
        .with("client", Client::default())
        .with("proxy", format_args!("environment"));
    assert_eq!(settings, expected);
    assert!(!holds_a_header(&format!("{settings:?}")) && !holds_a_password(&format!("{settings:?}")), "{settings:?}");

    // An `@` in the path may close a password holding an unencoded `/`: the authority is not
    // reported.
    let at_in_path = OtlpExporter::http().settings(&options("https://user:pass/word@collector:4318/v1", HEADERS[0]));
    assert!(format!("{at_in_path:?}").contains("\"https://[unreadable]\""), "{at_in_path:?}");
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

/// Equal endpoints are one module, and so are two that differ only in their userinfo or their
/// query. Planted breaks: the endpoint compared with its userinfo; with its query.
#[tokio::test]
async fn one_endpoint_is_one_module_whatever_its_userinfo_or_query() {
    let header = HEADERS[0];
    wired(Root(vec![otel("http://a:4318", header), otel("http://a:4318", header)])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec![otel("http://one:x@a:4318", header), otel("http://two:y@a:4318", header)]))
        .unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec![otel("http://a:4318?api_key=one", header), otel("http://a:4318?api_key=two", header)]))
        .unwrap_or_else(|errors| panic!("{errors}"));
}

/// Two endpoints differing only in their path are two collectors, refused under one identity,
/// and neither the password in the userinfo nor the one in the query appears in the refusal's
/// text or in the entry's fields. Neither is registered with the graph: the endpoint is no
/// secret, and the second registration never runs. Planted breaks: the endpoint reported
/// without its path; with its userinfo; with its query.
#[tokio::test]
async fn two_paths_under_one_identity_are_refused_without_the_credentials() {
    let at = |path: &str| format!("https://user:userinfo-credential-value@collector:4318{path}?api_key=query-credential-value");
    let errors = match wired(Root(vec![otel(&at("/one"), HEADERS[0]), otel(&at("/two"), HEADERS[0])])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two endpoint paths under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let fields: Vec<_> = settings.iter().map(sides).collect();
    assert_eq!(
        fields,
        [("endpoint", Some("https://collector:4318/one".to_owned()), Some("https://collector:4318/two".to_owned()))]
    );
    assert!(!holds_a_password(&format!("{fields:?}")), "{fields:?}");
    let text = errors.to_string();
    assert!(!holds_a_password(&text), "{text}");
    assert!(!holds_a_password(&format!("{errors:?}")), "{errors:?}");
}

/// The proxy is a setting, written as an endpoint is: two proxies under one identity are refused
/// naming `proxy`, and neither the password in a proxy's userinfo nor the one in its query
/// appears in the refusal. `off` and the environment are two more values. Planted breaks:
/// `OtlpExporter::settings` without the proxy; the proxy written as given.
#[tokio::test]
async fn two_proxies_under_one_identity_are_refused_without_their_credentials() {
    let through = |exporter: OtlpExporter| {
        Otel::new(options("http://a:4318", HEADERS[0]), exporter).unwrap_or_else(|error| panic!("the exporter builds: {error}"))
    };
    let proxy = |host: &str| {
        OtlpExporter::http().proxy(format!("http://user:userinfo-credential-value@{host}:3128?pass=query-credential-value"))
    };
    let errors = match wired(Root(vec![through(proxy("one")), through(proxy("two"))])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two proxies under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let fields: Vec<_> = settings.iter().map(sides).collect();
    assert_eq!(fields, [("proxy", Some("http://one:3128".to_owned()), Some("http://two:3128".to_owned()))]);
    assert!(!holds_a_password(&format!("{fields:?}")), "{fields:?}");
    let text = errors.to_string();
    assert!(!holds_a_password(&text), "{text}");
    assert!(!holds_a_password(&format!("{errors:?}")), "{errors:?}");

    let errors = match wired(Root(vec![through(OtlpExporter::http().no_proxy()), through(OtlpExporter::http())])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; no proxy and the environment's under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let fields: Vec<_> = settings.iter().map(sides).collect();
    assert_eq!(fields, [("proxy", Some("off".to_owned()), Some("environment".to_owned()))]);
}

/// The client is a setting. Planted break: `OtlpExporter::settings` without the client.
#[cfg(all(feature = "ureq", feature = "reqwest-blocking"))]
#[tokio::test]
async fn two_clients_under_one_identity_are_refused() {
    let over = |client: Client| {
        Otel::new(options("http://a:4318", HEADERS[0]), OtlpExporter::http_over(client))
            .unwrap_or_else(|error| panic!("the exporter builds: {error}"))
    };
    let errors = match wired(Root(vec![over(Client::Ureq), over(Client::ReqwestBlocking)])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two clients under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let fields: Vec<_> = settings.iter().map(sides).collect();
    assert_eq!(fields, [("client", Some("Ureq".to_owned()), Some("ReqwestBlocking".to_owned()))]);
}
