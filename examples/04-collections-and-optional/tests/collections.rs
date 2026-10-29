use ex_04_collections_and_optional::{
    AuditLog, ExtrasModule, Host, HostController, Mailer, MetricsPlugin, Plugin, TracingPlugin, builder,
};
use ex_support::{get, start};
use ulo::{App, injectable, module};

#[tokio::test]
async fn plugins_run_in_contribution_order_and_unbound_optionals_are_none() {
    let server = start(builder()).await;

    // Imports first, then the module's own entries in the order written.
    assert_eq!(get(server.addr, "/plugins").await.body, "extra,metrics,tracing");
    assert_eq!(get(server.addr, "/optional").await.body, "audit=false mailer=false");

    server.stop().await.unwrap();
}

#[injectable]
struct NullMailer;

impl Mailer for NullMailer {}

#[module(
    imports = [ExtrasModule],
    providers = [Host, AuditLog, NullMailer as dyn Mailer, into dyn Plugin: [TracingPlugin, MetricsPlugin]],
    controllers = [HostController],
)]
struct AuditedModule;

#[tokio::test]
async fn bound_optionals_are_some_and_the_written_order_changes_the_collection() {
    let server = start(App::builder(AuditedModule)).await;

    assert_eq!(get(server.addr, "/plugins").await.body, "extra,tracing,metrics");
    assert_eq!(get(server.addr, "/optional").await.body, "audit=true mailer=true");

    server.stop().await.unwrap();
}
