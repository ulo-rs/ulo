use ex_07_configured_modules::{AppConfig, ConfigModule, Primary, Replica, StoreModule, builder};
use ex_support::{get, start};
use ulo::{Dep, StartupError, WiringError, injectable, module};
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn each_keyed_instance_answers_with_its_own_configuration() {
    let server = start(builder()).await;

    assert_eq!(get(server.addr, "/config").await.body, "ledger");
    assert_eq!(get(server.addr, "/primary").await.body, "postgres://primary");
    assert_eq!(get(server.addr, "/replica").await.body, "postgres://replica");

    let primary = server.handle.module_keyed::<StoreModule, Primary>().unwrap();
    let replica = server.handle.module_keyed::<StoreModule, Replica>().unwrap();
    assert_eq!(primary.name().to_string(), "StoreModule @ Primary");
    assert_eq!(replica.name().to_string(), "StoreModule @ Replica");
    server.stop().await.unwrap();
}

/// Reads `AppConfig` without importing `ConfigModule`: only a global export reaches it.
#[injectable]
struct Banner {
    config: Dep<AppConfig>,
}

#[module(providers = [Banner], exports = [Banner])]
struct BannerModule;

#[module(imports = [ConfigModule, BannerModule])]
struct WithBanner;

#[tokio::test]
async fn a_global_export_reaches_a_module_that_does_not_import_it() {
    let app = ulo::App::builder(WithBanner).runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();
    assert_eq!(app.get::<Banner>().await.unwrap().config.name, "ledger");
}

#[module(imports = [StoreModule::for_root("same"), StoreModule::for_root("same")])]
struct Twice;

#[module(imports = [StoreModule::for_root("one"), StoreModule::for_root("two")])]
struct TwoConfigurations;

#[tokio::test]
async fn one_configuration_imported_twice_is_one_module() {
    let app = ulo::App::builder(Twice).runtime(Tokio::current()).wire().unwrap().connect().await.unwrap();
    assert_eq!(app.module::<StoreModule>().unwrap().name().to_string(), "StoreModule");
}

#[test]
fn two_unkeyed_configurations_export_one_key_twice_and_wiring_names_both() {
    let Err(StartupError::Wiring(errors)) = ulo::App::builder(TwoConfigurations).wire() else {
        panic!("wiring should fail");
    };
    let [WiringError::Ambiguous { key, module, sources, .. }] = errors.iter().collect::<Vec<_>>()[..] else {
        panic!("expected one ambiguity, got {errors}");
    };
    assert_eq!(key.to_string(), "Store");
    assert_eq!(module.to_string(), "TwoConfigurations");
    let names: Vec<String> = sources.iter().map(|(m, _)| m.to_string()).collect();
    assert_eq!(names, ["StoreModule", "StoreModule #2"]);
}
