//! The Redis adapter's settings, which `WsModule` compares: `redis` and the URL without its
//! userinfo. Nothing connects: `wire()` reads the settings alone.

use ulo::{App, Module, ModuleDef, ModuleIdentity, StartupError, WiringError, WiringErrors};
use ulo_ws::{BroadcastAdapter, WsModule};
use ulo_ws_redis::Redis;

const PASSWORD: &str = "redis-credential-value";

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

fn wired(root: Root) -> Result<(), WiringErrors> {
    match App::builder(root).runtime(ulo_runtime_tokio::Tokio::current()).wire() {
        Ok(_) => Ok(()),
        Err(StartupError::Wiring(errors)) => Err(errors),
        Err(other) => panic!("expected wiring errors, got: {other}"),
    }
}

/// Planted breaks: `Redis::settings` left at the trait's default; the URL written with its
/// userinfo.
#[test]
fn the_settings_name_redis_and_the_url_without_its_userinfo() {
    let settings = Redis::url("redis://user:redis-credential-value@cache:6379/0").settings();
    assert_eq!(settings, ulo::ModuleSettings::new().with("broadcast", format_args!("redis")).with("url", format_args!("redis://cache:6379/0")));
}

/// Two URLs under one identity are refused naming `url`, and the userinfo appears nowhere in the
/// refusal. Planted break: the URL written with its userinfo.
#[tokio::test]
async fn two_urls_under_one_identity_are_refused_without_the_userinfo() {
    let errors = match wired(Root(vec!["redis://:redis-credential-value@a:6379", "redis://:redis-credential-value@b:6379"])) {
        Err(errors) => errors,
        Ok(()) => panic!("wired; two Redis URLs under one identity should be refused"),
    };
    assert_eq!(errors.len(), 1, "{errors}");
    let Some(WiringError::DifferingSettings { settings, .. }) = errors.iter().next() else {
        panic!("expected `DifferingSettings`:\n{errors}");
    };
    let settings: Vec<_> = settings.iter().map(|s| (s.name, s.first.clone(), s.second.clone())).collect();
    assert_eq!(settings, [("url", Some("redis://a:6379".to_owned()), Some("redis://b:6379".to_owned()))]);
    assert!(!format!("{settings:?}").contains(PASSWORD), "{settings:?}");
    let text = errors.to_string();
    assert!(!text.contains(PASSWORD), "{text}");
}

/// One URL is one module, and so are two differing only in their userinfo.
#[tokio::test]
async fn one_url_is_one_module_whatever_its_userinfo() {
    wired(Root(vec!["redis://a:6379", "redis://a:6379"])).unwrap_or_else(|errors| panic!("{errors}"));
    wired(Root(vec!["redis://:one@a:6379", "redis://:two@a:6379"])).unwrap_or_else(|errors| panic!("{errors}"));
}
