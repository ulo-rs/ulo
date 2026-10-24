//! Declared metadata on an RPC handler: the `#[routes]` impl's declarations apply to every handler
//! in it, the method's own come first and win per type, and every declaration is
//! listable. Capability: declared metadata on RPC (G15).

use std::sync::Arc;
use std::time::Duration;

use ulo::{App, AppHandle, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_rpc::{RpcClient, RpcCx};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);

/// A declaration with a value, as a role requirement.
#[derive(Debug)]
struct Roles(&'static str);

/// A declaration with no value, a marker.
struct Audited;

fn describe(cx: &RpcCx) -> String {
    let info = cx.exec().handler().expect("the handler's info is set on the execution");
    let winner = info.meta::<Roles>().map_or("-", |roles| roles.0);
    let all: Vec<&str> = info.meta_all::<Roles>().map(|roles| roles.0).collect();
    format!("{winner}|{}|{}", all.join(","), info.meta::<Audited>().is_some())
}

#[injectable]
struct Guarded;

#[routes]
#[meta(Roles("user"), Audited)]
impl Guarded {
    #[ulo_rpc::message("meta.inherits")]
    fn inherits(&self, cx: RpcCx) -> String {
        describe(&cx)
    }

    #[ulo_rpc::message("meta.overrides")]
    #[meta(Roles("admin"))]
    fn overrides(&self, cx: RpcCx) -> String {
        describe(&cx)
    }

    #[ulo_rpc::message("meta.twice")]
    #[meta(Roles("first"), Roles("second"))]
    fn twice(&self, cx: RpcCx) -> String {
        describe(&cx)
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Guarded>();
    }
}

struct Running {
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: RpcClient,
}

async fn start() -> Running {
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");
    Running { handle, serving, client }
}

impl Running {
    async fn described(&self, pattern: &str) -> String {
        let call = self.client.request::<u32, String>(pattern, &0);
        match tokio::time::timeout(PATIENCE, call).await {
            Ok(Ok(text)) => text,
            Ok(Err(error)) => panic!("{pattern} failed: {error}"),
            Err(_) => panic!("{pattern} did not answer within {PATIENCE:?}"),
        }
    }

    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

#[tokio::test]
async fn a_handler_inherits_the_impl_s_declarations() {
    let app = start().await;
    assert_eq!(app.described("meta.inherits").await, "user|user|true");
    app.stop().await;
}

#[tokio::test]
async fn the_method_s_declaration_wins_per_type_and_the_impl_s_stays_listable_after_it() {
    let app = start().await;
    assert_eq!(app.described("meta.overrides").await, "admin|admin,user|true");
    app.stop().await;
}

#[tokio::test]
async fn two_declarations_of_one_type_on_a_method_are_both_listed_the_first_winning() {
    let app = start().await;
    assert_eq!(app.described("meta.twice").await, "first|first,second,user|true");
    app.stop().await;
}
