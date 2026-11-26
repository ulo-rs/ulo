use std::sync::Arc;

use ex_02_providers::{AppModule, Counter, Greeter, HelloController, Settings};
use ex_support::get;
use ulo::testing::TestApp;
use ulo::{ConnectError, Dep, StartupError, module};
use ulo_http_hyper::Server;
use ulo_runtime_tokio::Tokio;
use ulo_runtime_tokio::testing::start;

#[tokio::test]
async fn two_requests_read_one_singletons_counter() {
    let served = start(TestApp::of(AppModule), Server::new("127.0.0.1:0")).await.unwrap();

    assert_eq!(get(served.addr(), "/").await.body, "Hello #1");
    assert_eq!(get(served.addr(), "/").await.body, "Hello #2");

    let counter: Dep<Counter> = served.handle().get().await.unwrap();
    assert_eq!(counter.next(), 3);
    let again: Dep<Counter> = served.handle().get().await.unwrap();
    assert!(Arc::ptr_eq(counter.arc(), again.arc()));

    served.close().await.unwrap();
}

#[module(
    providers = [Counter, Greeter, Settings { salutation: "" }],
    controllers = [HelloController],
)]
struct EmptySalutationModule;

#[tokio::test]
async fn a_failing_constructor_fails_connect_naming_the_key_and_module() {
    let Err(error) = ulo::App::builder(EmptySalutationModule)
        .runtime(Tokio::current())
        .wire()
        .unwrap()
        .connect()
        .await
    else {
        panic!("connect should fail");
    };
    let StartupError::Connect { error: connect_error, .. } = &error else {
        panic!("expected a construct failure, got {error}");
    };
    let ConnectError::Construct { key, module, reason, .. } = &**connect_error else {
        panic!("expected a construct failure, got {error}");
    };
    assert_eq!(key.to_string(), "Greeter");
    assert_eq!(module.to_string(), "EmptySalutationModule");
    assert_eq!(reason.to_string(), "the salutation is empty");
}
