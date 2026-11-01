//! A generated `Construct` with a `Session<T>` field passes the declared-read check: on a gateway
//! with no session, or a session of another type, it answers `NotFound` with `LookupKind::Input`
//! naming `Session<T>`, as `Session`'s documentation states, not an undeclared read.

mod support;

use serde_json::json;
use ulo::scope::Auto;
use ulo::{
    BoxError, Construct, ConstructError, Controller, Dependencies, FailureReason, HandlerSpec, LookupError, Module, ModuleDef,
    ModuleIdentity, Mount, Permanent, Resolver, UndeclaredRead, injectable,
};
use ulo_ws::{GatewayConfig, GatewayHooks, GatewaySettings, Reply, Session, SessionFactory, Ws, WsCx, WsHandler, WsModule};

use support::{Running, hang_up, next_json, send_json};

#[injectable(execution)]
struct NeedsSession {
    _s: Session<String>,
}

fn classify(e: &LookupError) -> String {
    match e {
        LookupError::NotFound { key, kind } => format!("NotFound {kind:?} {key}"),
        LookupError::Construct { key, reason: FailureReason::Errored(r) } => {
            let undeclared = r
                .downcast_ref::<Permanent>()
                .and_then(|p| std::error::Error::source(p))
                .and_then(|s| s.downcast_ref::<UndeclaredRead>())
                .map(|u| u.to_string());
            format!("Construct {key} undeclared={undeclared:?}")
        }
        other => format!("other: {other}"),
    }
}

async fn probe(cx: WsCx) -> Result<Reply, BoxError> {
    let answer = match cx.exec().get::<NeedsSession>().await {
        Ok(_) => "built".to_string(),
        Err(e) => classify(&e),
    };
    Ok(cx.reply(&answer)?)
}

macro_rules! gateway {
    ($name:ident, $path:literal, $session:expr) => {
        struct $name;
        impl Construct for $name {
            type Scope = Auto;
            fn dependencies(_: &mut Dependencies) {}
            async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
                Ok($name)
            }
        }
        impl GatewayConfig for $name {
            fn settings() -> GatewaySettings {
                GatewaySettings::at($path)
            }
            fn session() -> SessionFactory {
                $session
            }
            fn mount_gateway(m: &mut Mount<'_>) {
                GatewayHooks::<Self>::new().mount(m);
            }
        }
        impl Controller for $name {
            fn mount(m: &mut Mount<'_>) {
                m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
                m.handler(HandlerSpec::<Ws, _>::new("probe", WsHandler::new(probe)).route("probe"));
            }
        }
    };
}

gateway!(NoSession, "/none", SessionFactory::none());
gateway!(U32Session, "/u32", SessionFactory::default_of::<u32>());
gateway!(StringSession, "/string", SessionFactory::default_of::<String>());

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.provide::<NeedsSession>();
        m.controller::<NoSession>();
        m.controller::<U32Session>();
        m.controller::<StringSession>();
    }
}

#[tokio::test]
async fn a_session_field_without_its_session_is_an_unseeded_input_not_an_undeclared_read() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let unseeded = "NotFound Input Session<String>";
    for (path, expected) in [("/none", unseeded), ("/u32", unseeded), ("/string", "built")] {
        let mut socket = app.connect(path, &[]).await;
        send_json(&mut socket, &json!({ "event": "probe", "id": 1, "data": null })).await;
        assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": expected }), "{path}");
        hang_up(socket).await;
    }
    app.stop().await;
}
