use ex_08_dynamic_and_lazy::{BuildLog, FeatureController, Reports, ReportsModule, builder, feature_module};
use ex_support::{get, start};
use ulo::{Dep, LoadError, LoadRefusal, LookupError, module};

#[tokio::test]
async fn a_runtime_built_module_answers_with_the_value_it_was_built_from() {
    let server = start(builder("beta")).await;
    assert_eq!(get(server.addr, "/feature").await.body, "beta");
    let feature = server.handle.module_of(&feature_module("beta".to_owned())).unwrap();
    assert_eq!(feature.name().to_string(), "FeatureModule");
    server.stop().await.unwrap();
}

#[tokio::test]
async fn a_lazy_module_builds_nothing_until_it_is_loaded_and_loads_once() {
    let server = start(builder("alpha")).await;
    let log: Dep<BuildLog> = server.handle.get().await.unwrap();

    assert!(log.entries().is_empty());
    assert!(matches!(server.handle.get::<Reports>().await, Err(LookupError::NotFound { .. })));

    let reports = server.handle.load(ReportsModule).await.unwrap();
    assert_eq!(log.entries(), ["Reports"]);
    assert_eq!(reports.name().to_string(), "ReportsModule");
    assert_eq!(reports.get::<Reports>().await.unwrap().title(), "quarterly");

    let again = server.handle.load(ReportsModule).await.unwrap();
    assert_eq!(again.get::<Reports>().await.unwrap().title(), "quarterly");
    assert_eq!(log.entries(), ["Reports"], "an equal identity loads once");
    server.stop().await.unwrap();
}

#[module(controllers = [FeatureController])]
struct RoutesModule;

#[tokio::test]
async fn a_lazy_module_with_a_controller_is_refused_naming_the_module() {
    let server = start(builder("alpha")).await;
    let Err(LoadError::Refused(LoadRefusal::Controllers { module })) = server.handle.load(RoutesModule).await else {
        panic!("the load should be refused");
    };
    assert_eq!(module.to_string(), "RoutesModule");
    server.stop().await.unwrap();
}
