//! `connect_within` bounds a server's `listen`, which the link's own tests hold past the bound
//! against a broker (`link.rs`, `own_consumers`). A zero bound is refused by `prepare`, which
//! reaches no broker, so none is needed here.

use std::time::Duration;

use ulo::{App, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::Link;
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

/// Planted break: `prepare` not reading `connect_within`.
#[tokio::test(flavor = "multi_thread")]
async fn a_zero_connect_within_is_refused_by_prepare() {
    let app = App::builder(Root).runtime(Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let mut zero = Kafka::brokers("127.0.0.1:1").connect_within(Duration::ZERO);
    let refused = zero.prepare(&app.handle()).await.err().unwrap_or_else(|| panic!("`prepare` accepted a zero `connect_within`"));
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");

    let mut short = Kafka::brokers("127.0.0.1:1").connect_within(Duration::from_millis(1));
    short.prepare(&app.handle()).await.unwrap_or_else(|refused| panic!("`prepare` refused a nonzero `connect_within`: {refused}"));
}
