use std::sync::Arc;

use ex_02_providers::{Counter, Greeter, HelloController, Settings, builder};
use ex_support::{get, start};
use ulo::{ConnectError, Dep, StartupError, module};
use ulo_runtime_tokio::Tokio;

#[tokio::test]
async fn two_requests_read_one_singletons_counter() {
    let server = start(builder()).await;

    assert_eq!(get(server.addr, "/").await.body, "Hello #1");
    assert_eq!(get(server.addr, "/").await.body, "Hello #2");

    let counter: Dep<Counter> = server.handle.get().await.unwrap();
    assert_eq!(counter.next(), 3);
    let again: Dep<Counter> = server.handle.get().await.unwrap();
    assert!(Arc::ptr_eq(counter.arc(), again.arc()));

    server.stop().await.unwrap();
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
    let StartupError::Connect(ConnectError::Construct { key, module, reason }) = &error else {
        panic!("expected a construct failure, got {error}");
    };
    assert_eq!(key.to_string(), "Greeter");
    assert_eq!(module.to_string(), "EmptySalutationModule");
    assert_eq!(reason.to_string(), "the salutation is empty");
}
