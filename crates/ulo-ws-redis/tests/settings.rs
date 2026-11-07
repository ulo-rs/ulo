//! The Redis adapter's settings, which `WsModule` compares: `redis`, where the URL connects,
//! canonical and never with its userinfo, query, fragment or database, the database, the protocol
//! and, for TLS, whether the certificate is verified, each as the redis client reads the URL, and
//! `connect_within`. Nothing connects: `wire()` reads the settings alone, and `prepare` reads the
//! URL.

use std::time::Duration;

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, WiringError, WiringErrors};
use ulo_ws::{BroadcastAdapter, WsModule};
use ulo_ws_redis::Redis;

/// The password each URL carries in its userinfo, and the one it carries in its query.
const PASSWORDS: [&str; 2] = ["userinfo-credential-value", "query-credential-value"];

fn holds_a_password(text: &str) -> bool {
    PASSWORDS.iter().any(|password| text.contains(password))
}

/// Imports `WsModule` over each URL's adapter, in order.
struct Root(Vec<&'static str>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Root>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for url in &self.0 {
            m.import(WsModule::for_root().broadcast(Redis::url(*url)));
        }
    }
}

/// Imports `WsModule` over each adapter, in order.
struct Adapters(Vec<Redis>);

impl Module for Adapters {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Adapters>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        for adapter in &self.0 {
            m.import(WsModule::for_root().broadcast(adapter.clone()));
        }
    }
}

fn wired(root: impl Module + 'static) -> Result<(), WiringErrors> {
    match App::builder(root).runtime(ulo_runtime_tokio::Tokio::current()).wire() {
        Ok(_) => Ok(()),
        Err(StartupError::Wiring(errors)) => Err(errors),
        Err(other) => panic!("expected wiring errors, got: {other}"),
    }
}

/// Planted breaks: `Redis::settings` left at the trait's default; the URL written with its
/// userinfo; the URL written with its query; the URL written with its database.
#[test]
fn the_settings_name_redis_and_the_url_without_its_userinfo_or_query() {
    let settings = Redis::url("redis://user:userinfo-credential-value@cache:6379/0?pass=query-credential-value#f").settings();
    let expected = ulo::ModuleSettings::new()
        .with("broadcast", format_args!("redis"))
        .with("url", format_args!("redis://cache:6379"))
        .with("database", 0)
        .with("protocol", format_args!("resp2"))
        .with("connect_within", Duration::from_secs(30));
    assert_eq!(settings, expected);
    let socket = Redis::url("redis+unix:///run/redis.sock?pass=query-credential-value&db=1").settings();
    assert!(format!("{socket:?}").contains("\"redis+unix:///run/redis.sock\""), "{socket:?}");
}

/// Two URLs under one identity are refused naming `url`, and neither the password in the
/// userinfo nor the one in the query appears in the refusal's text or in the entry's fields.
/// Neither password is registered with the graph: the second registration's never runs.
/// Planted breaks: the URL written with its userinfo; the URL written with its query.
#[tokio::test]
async fn two_urls_under_one_identity_are_refused_without_their_credentials() {
    let errors = match wired(Root(vec![
        "redis://:userinfo-credential-value@a:6379?pass=query-credential-value",
        "redis://:userinfo-credential-value@b:6379?pass=query-credential-value",
    ])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two Redis URLs under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("url", Some("redis://a:6379".to_owned()), Some("redis://b:6379".to_owned()))]);
    assert!(!holds_a_password(&format!("{settings:?}")), "{settings:?}");
    let text = errors.to_string();
    assert!(!holds_a_password(&text), "{text}");
    assert!(!holds_a_password(&format!("{errors:?}")), "{errors:?}");
}

/// One URL is one module, and so are two differing only in their userinfo, in a query parameter
/// that selects nothing, or in spelling the default port, database or protocol out. Planted
/// breaks: the URL written as given; the URL written with its database.
#[tokio::test]
async fn one_url_is_one_module_whatever_its_userinfo_or_query() {
    wired(Root(vec!["redis://a:6379", "redis://a:6379"])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec!["redis://:one@a:6379", "redis://:two@a:6379"])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec!["redis://a:6379", "redis://a:6379/0"])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec!["redis://a", "redis://a:6379/?protocol=resp2&name=x"])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec!["rediss://a", "rediss://a:6379/"])).unwrap_or_else(|errors| panic!("{errors}"));
}

/// `#insecure` turns certificate verification off, so two TLS URLs differing only there are
/// refused naming `tls_verify` rather than resolved to whichever came first. Planted break:
/// `Redis::settings` without `tls_verify`.
#[tokio::test]
async fn verified_and_unverified_tls_under_one_identity_are_refused() {
    let errors = match wired(Root(vec!["rediss://a:6380", "rediss://a:6380/#insecure"])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; a verified and an unverified TLS URL under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("tls_verify", Some("true".to_owned()), Some("false".to_owned()))]);
}

/// The `protocol` query parameter selects RESP3, so two URLs differing only there are refused
/// naming `protocol`. Planted break: `Redis::settings` without `protocol`.
#[tokio::test]
async fn two_protocols_under_one_identity_are_refused() {
    let errors = match wired(Root(vec!["redis://a:6379?protocol=resp2", "redis://a:6379?protocol=resp3"])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two protocols under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("protocol", Some("resp2".to_owned()), Some("resp3".to_owned()))]);
}

/// The path identifies the database, so two URLs differing only there are refused naming
/// `database`. Planted break: `Redis::settings` without `database`.
#[tokio::test]
async fn two_databases_under_one_identity_are_refused() {
    let errors = match wired(Root(vec!["redis://a:6379/0", "redis://a:6379/1"])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two Redis databases under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("database", Some("0".to_owned()), Some("1".to_owned()))]);
}

/// A Unix socket's database is its `db` query parameter, which the URL setting leaves out: two
/// sockets differing only there are refused naming `database`. Planted break: `Redis::settings`
/// without `database`.
#[tokio::test]
async fn two_unix_socket_databases_under_one_identity_are_refused() {
    let errors = match wired(Root(vec!["redis+unix:///run/redis.sock?db=1", "redis+unix:///run/redis.sock?db=2"])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two Redis databases on one socket under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("database", Some("1".to_owned()), Some("2".to_owned()))]);
    wired(Root(vec!["redis+unix:///run/redis.sock?db=1&pass=a", "redis+unix:///run/redis.sock?pass=b&db=1"]))
        .unwrap_or_else(|errors| panic!("one database on one socket: {errors}"));
}

/// A `db` that is not a number is a URL the redis client does not read: the settings say so and
/// `prepare` refuses the adapter. Planted break: `prepare` not reading the URL.
#[tokio::test]
async fn a_database_that_is_not_a_number_is_unreadable_and_refused_by_prepare() {
    let adapter = Redis::url("redis+unix:///run/redis.sock?db=one&pass=query-credential-value");
    let settings = format!("{:?}", adapter.settings());
    assert!(settings.contains("\"database\": \"[unreadable]\""), "{settings}");
    assert!(settings.contains("\"protocol\": \"[unreadable]\""), "{settings}");
    assert!(settings.contains("\"url\": \"redis+unix:///run/redis.sock\""), "{settings}");
    let refused = adapter.prepare().err().unwrap_or_else(|| panic!("prepare accepted a database that is not a number"));
    let text = refused.to_string();
    assert!(text.contains("the Redis broadcast adapter's URL is not one the redis client reads"), "{text}");
    assert!(text.contains("Invalid database number"), "{text}");
    assert!(!holds_a_password(&text), "{text}");
    Redis::url("redis+unix:///run/redis.sock?db=3").prepare().unwrap_or_else(|error| panic!("a numeric database: {error}"));
}

/// The subscription bound is a setting: a second import with another is refused rather than
/// running on the first's. Planted break: `Redis::settings` without `connect_within`.
#[tokio::test]
async fn two_connect_bounds_under_one_identity_are_refused() {
    let adapter = |seconds: u64| Redis::url("redis://a:6379").connect_within(Duration::from_secs(seconds));
    let errors = match wired(Adapters(vec![adapter(5), adapter(10)])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two connect bounds under one identity should be refused"),
    };
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("connect_within", Some("5s".to_owned()), Some("10s".to_owned()))]);
}
