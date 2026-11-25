//! `{:?}` of an `UpgradeHead` names the method, the path and the header names, and prints no
//! header value and no query.

mod support;

use serde_json::json;
use ulo::scope::Auto;
use ulo::{
    BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Handlers, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver,
};
use ulo_ws::{GatewayConfig, GatewayHooks, GatewaySettings, Reply, SessionFactory, UpgradeHead, Ws, WsCx, WsHandler, WsModule};

use support::{Running, hang_up, next_json, send_json};

struct Echo;

impl Construct for Echo {
    type Scope = Auto;
    fn dependencies(d: &mut Dependencies) {
        d.mark::<Self>();
    }
    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Echo)
    }
}

impl GatewayConfig for Echo {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/echo")
    }
    fn session() -> SessionFactory {
        SessionFactory::none()
    }
    fn mount_gateway(m: &mut Mount<'_>) {
        GatewayHooks::<Self>::new().mount(m);
    }
}

impl Controller for Echo {}

async fn shown(cx: WsCx) -> Result<Reply, BoxError> {
    let head = cx.exec().get::<UpgradeHead>().await?;
    Ok(cx.reply(&format!("{:?}", *head))?)
}

impl Handlers for Echo {
    fn mount(m: &mut Mount<'_>) {
        m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
        m.handler(HandlerSpec::<Ws, _>::new("shown", WsHandler::new(shown)).route("shown"));
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Echo>();
    }
}

#[tokio::test]
async fn an_upgrade_head_s_debug_output_carries_no_header_value_and_no_query() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let headers = [("Authorization", "Bearer s3cr3t"), ("Cookie", "sid=abc")];
    let mut socket = app.connect("/echo?token=q7", &headers).await;
    send_json(&mut socket, &json!({ "event": "shown", "id": 1, "data": null })).await;
    let reply = next_json(&mut socket).await;
    let text = reply["data"].as_str().expect("the handler replies with the text").to_owned();
    for secret in ["s3cr3t", "sid=abc", "q7"] {
        assert!(!text.contains(secret), "{secret} appears in {text}");
    }
    assert!(text.contains("authorization") && text.contains("cookie"), "header names are kept: {text}");
    hang_up(socket).await;
    app.stop().await;
}
