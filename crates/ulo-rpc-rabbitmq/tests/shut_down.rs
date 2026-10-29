//! A link on a tokio runtime that has shut down, on `amqp://`: its server's `listen()` fails
//! `Configure`, permanent, with the refusal's text, and each client call fails `Unavailable` with
//! `reason: "runtime_shut_down"`, naming the link. A runtime that shuts down after `prepare` fails
//! `listen` with a `Permanent` `RuntimeShutDown`. Nothing listens at the address, and the link
//! fails before it would connect, so no broker is needed.

use std::sync::Arc;

use tokio::runtime::{Builder, Handle};
use ulo::{App, Module, ModuleDef, ModuleIdentity, Permanent, RuntimeShutDown, StartupError};
use ulo_rpc::{Link, RpcClient, Server};
use ulo_rpc_rabbitmq::RabbitMq;
use ulo_runtime_tokio::Tokio;
use ulo_transport::ErrorKind;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// The handle of a runtime that has shut down, dropped on a thread of its own: dropping a runtime
/// inside another's `block_on` panics.
fn stopped() -> Handle {
    std::thread::spawn(|| {
        let runtime = Builder::new_current_thread().build().expect("a runtime to shut down");
        let handle = runtime.handle().clone();
        drop(runtime);
        handle
    })
    .join()
    .expect("the runtime shuts down")
}

#[test]
fn a_link_on_a_shut_down_runtime_answers_runtime_shut_down() {
    let running = Builder::new_current_thread().enable_all().build().expect("a tokio runtime for the app");
    running.block_on(async {
        let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
        let serving = RabbitMq::url("amqp://127.0.0.1:1").with_handle(stopped());
        let error = app.bind(Server::new(serving)).listen().await.err().expect("a server on a runtime that had shut down listened");
        assert!(matches!(error, StartupError::Configure(_)), "the server failed other than as a `Configure` refusal: {error:?}");
        assert!(ulo::is_permanent(&error), "a server refused on a runtime that has shut down is retryable: {error}");
        assert!(error.to_string().contains("the runtime has shut down"), "the server's refusal does not name the shutdown: {error}");

        let client = RpcClient::new(RabbitMq::url("amqp://127.0.0.1:1").with_handle(stopped()), Arc::new(Tokio::current())).expect("the link is usable");
        for _ in 0..2 {
            let call = client.request::<_, u32>("rabbitmq.shut_down", &()).await.expect_err("a call on a runtime that had shut down answered");
            assert_eq!((call.kind(), call.reason()), (ErrorKind::Unavailable, Some(ulo::RUNTIME_SHUT_DOWN)), "{}", call.message());
            assert!(call.message().contains("the rabbitmq link"), "the call does not name the link: {}", call.message());
        }
    });
}

#[test]
fn a_runtime_that_shuts_down_after_prepare_fails_listen_as_a_permanent_refusal() {
    let running = Builder::new_current_thread().enable_all().build().expect("a tokio runtime for the app");
    running.block_on(async {
        let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
        let linked = Builder::new_multi_thread().worker_threads(1).enable_all().build().expect("the link's runtime");
        let mut serving = RabbitMq::url("amqp://127.0.0.1:1").with_handle(linked.handle().clone());
        serving.prepare(&app.handle()).await.expect("the link prepares on a running runtime");
        std::thread::spawn(move || drop(linked)).join().expect("the link's runtime shuts down");
        let failed = serving.listen(&[]).await.err().expect("a link listened on a runtime that had shut down");
        let permanent = failed.downcast_ref::<Permanent>().unwrap_or_else(|| panic!("the refusal is not `Permanent`: {failed:?}"));
        let inner = std::error::Error::source(permanent).expect("the `Permanent` wraps the refusal");
        assert!(inner.is::<RuntimeShutDown>(), "the `Permanent` does not hold a `RuntimeShutDown`: {failed:?}");
    });
}
