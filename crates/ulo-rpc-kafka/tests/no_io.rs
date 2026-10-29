//! Kafka does not register sockets with tokio: librdkafka owns its connections on threads of its
//! own, so a runtime built without its IO driver does not refuse the link, where a client takes it
//! or in a server's `prepare`. Neither reaches the network, so no broker is needed.

use std::sync::Arc;

use tokio::runtime::Builder;
use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{Link, RpcClient};
use ulo_rpc_kafka::Kafka;
use ulo_runtime_tokio::Tokio;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runtime_without_io_does_not_refuse_the_link() {
    let runtime = Builder::new_multi_thread().worker_threads(1).enable_time().build().expect("a runtime without IO");
    let client = RpcClient::new(Kafka::brokers("127.0.0.1:1").with_handle(runtime.handle().clone()), Arc::new(Tokio::current()));
    assert!(client.is_ok(), "a client over Kafka was refused on a runtime without IO: {:?}", client.err().map(|refused| refused.to_string()));

    let mut serving = Kafka::brokers("127.0.0.1:1").with_handle(runtime.handle().clone());
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let prepared = serving.prepare(&app.handle()).await;
    runtime.shutdown_background();
    assert!(prepared.is_ok(), "`prepare` refused a runtime without IO: {:?}", prepared.err().map(|refused| refused.to_string()));
}
