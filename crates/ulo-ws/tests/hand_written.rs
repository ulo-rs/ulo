//! An envelope gateway written without the attributes, through `GatewayConfig`, `GatewayHooks`,
//! `WsHandler` and `HandlerSpec`: its connect hook, a unary answer and a streamed one answer what
//! the attribute-written twin answers, with the same `HandlerInfo`; and a message handler mounted
//! with no event is refused naming its gateway.

mod support;

use futures_util::stream::{self, Stream};
use serde_json::json;
use ulo::scope::Auto;
use ulo::{
    App, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver, Shape, StartupError, TypeName, injectable, routes,
};
use ulo_ws::{
    CallError, ConnectCx, ConnectRefused, ErrorKind, FromCall, GatewayConfig, GatewayHooks, GatewaySettings, OnConnect, Payload,
    Reply, Ws, WsCx, WsHandler, WsModule,
};

use support::{Running, close_frame, hang_up, next_json, send_json, upgrade};

/// The upgrade header a connection is refused for.
const REFUSE: &str = "x-refuse";

fn refusal(cx: &ConnectCx) -> Result<(), ConnectRefused> {
    match cx.head().headers().get(REFUSE) {
        Some(_) => Err(ConnectRefused::kind(ErrorKind::Forbidden, "not today")),
        None => Ok(()),
    }
}

#[injectable]
struct Macro;

#[routes]
#[ulo_ws::gateway(path = "/m")]
impl Macro {
    #[ulo_ws::message("echo")]
    fn echo(&self, Payload(text): Payload<String>) -> String {
        format!("echo:{text}")
    }

    #[ulo_ws::message("count")]
    fn count(&self, Payload(n): Payload<u32>) -> impl Stream<Item = Result<u32, CallError>> + use<> {
        stream::iter((1..=n).map(Ok))
    }
}

impl OnConnect for Macro {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        refusal(cx)
    }
}

struct Hand;

impl Construct for Hand {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Hand)
    }
}

impl OnConnect for Hand {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        refusal(cx)
    }
}

impl GatewayConfig for Hand {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/h")
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        GatewayHooks::<Self>::new().on_connect().mount(m);
    }
}

async fn echo(cx: WsCx) -> Result<Reply, BoxError> {
    let Payload(text) = <Payload<String> as FromCall<Ws>>::from_call(&cx).await?;
    cx.exec().get::<Hand>().await?;
    Ok(cx.reply(&format!("echo:{text}"))?)
}

async fn count(cx: WsCx) -> Result<Reply, BoxError> {
    let Payload(n) = <Payload<u32> as FromCall<Ws>>::from_call(&cx).await?;
    Ok(cx.reply_stream(stream::iter((1..=n).map(Ok::<_, CallError>))))
}

impl Controller for Hand {
    fn mount(m: &mut Mount<'_>) {
        m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
        m.handler(HandlerSpec::<Ws, _>::new("echo", WsHandler::new(echo)).route("echo"));
        m.handler(HandlerSpec::<Ws, _>::new("count", WsHandler::new(count)).route("count"));
    }
}

struct Root(fn(&mut ModuleDef<'_>));

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        (self.0)(m);
    }
}

fn both(m: &mut ModuleDef<'_>) {
    m.controller::<Macro>();
    m.controller::<Hand>();
}

#[tokio::test]
async fn a_hand_written_gateway_answers_as_its_attribute_written_twin() {
    let app = Running::start(Root(both), ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    for (path, controller) in [("/m", TypeName::of::<Macro>()), ("/h", TypeName::of::<Hand>())] {
        let mut socket = app.connect(path, &[]).await;
        send_json(&mut socket, &json!({ "event": "echo", "id": 1, "data": "hi" })).await;
        assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "data": "echo:hi" }), "{path}");
        send_json(&mut socket, &json!({ "event": "count", "id": 2, "data": 2 })).await;
        assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "data": 1 }), "{path}");
        assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "data": 2 }), "{path}");
        assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "complete": true }), "{path}");
        hang_up(socket).await;

        let mut refused = upgrade(app.addr, path, &[(REFUSE, "1")]).await.socket.expect("the upgrade switches before the hook refuses");
        assert_eq!(close_frame(&mut refused).await, Some((1008, String::from("not today"))), "{path}");

        let handlers = app.handle.handlers();
        let ours: Vec<_> = handlers.iter().filter(|info| info.controller().key().type_name() == controller).collect();
        let connect = ours.iter().find(|info| info.transport() == "ws_connect").expect("the connect handler");
        assert_eq!(connect.route(), Some(path), "{path}");
        for (name, event) in [("echo", "echo"), ("count", "count")] {
            let info = ours.iter().find(|info| info.transport() == "ws" && info.name() == name).expect("the message handler");
            assert_eq!(info.route(), Some(event), "{path}");
            assert_eq!(info.prefix(), None, "{path}");
            assert_eq!(info.shape(), Shape::Unary, "{path}");
            assert_eq!(info.entries().count(), 0, "{path}");
        }
        assert_eq!(ours.len(), 3, "{path}");
    }
    app.stop().await;
}

struct Eventless;

impl Construct for Eventless {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Eventless)
    }
}

impl GatewayConfig for Eventless {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/eventless")
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        GatewayHooks::<Self>::new().mount(m);
    }
}

async fn nothing(_cx: WsCx) -> Result<Reply, BoxError> {
    Ok(Reply::None)
}

impl Controller for Eventless {
    fn mount(m: &mut Mount<'_>) {
        m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
        m.handler(HandlerSpec::<Ws, _>::new("lost", WsHandler::new(nothing)));
    }
}

#[tokio::test]
async fn a_message_handler_mounted_with_no_event_is_refused_naming_its_gateway() {
    let refused = App::builder(Root(|m| {
        m.controller::<Eventless>();
    }))
    .runtime(ulo_runtime_tokio::Tokio::current())
    .wire()
    .expect("the app wires")
    .connect()
    .await
    .expect("the app connects")
    .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
    .listen()
    .await;
    let Err(error) = refused else { panic!("listen() accepted a message handler with no event") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains("Eventless"), "{text}");
    assert!(text.contains("its message handler `lost` is mounted with no event; `HandlerSpec::route` sets it"), "{text}");
}

struct Gatewayless;

impl Construct for Gatewayless {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Gatewayless)
    }
}

impl Controller for Gatewayless {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Ws, _>::new("echo", WsHandler::new(nothing)).route("echo"));
    }
}

struct Valueless;

impl Construct for Valueless {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Valueless)
    }
}

impl GatewayConfig for Valueless {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/valueless")
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        GatewayHooks::<Self>::new().mount(m);
    }
}

impl Controller for Valueless {
    fn mount(m: &mut Mount<'_>) {
        m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
        m.handler(HandlerSpec::<Ws, _>::new("five", 5u32).route("five"));
        m.handler(HandlerSpec::<Ws, _>::new("cancel", WsHandler::new(nothing)).route("cancel"));
    }
}

#[tokio::test]
async fn a_message_handler_with_no_gateway_not_a_ws_handler_or_spelling_cancel_is_refused() {
    let refused = App::builder(Root(|m| {
        m.controller::<Gatewayless>();
        m.controller::<Valueless>();
    }))
    .runtime(ulo_runtime_tokio::Tokio::current())
    .wire()
    .expect("the app wires")
    .connect()
    .await
    .expect("the app connects")
    .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
    .listen()
    .await;
    let Err(error) = refused else { panic!("listen() accepted message handlers no gateway reaches") };
    assert!(matches!(error, StartupError::Configure(_)), "{error}");
    let text = error.to_string();
    assert!(text.contains("Gatewayless::echo` is mounted for WebSocket on a controller that mounted no gateway"), "{text}");
    assert!(text.contains("its message handler `five` is mounted with a handler value that is not a `ulo_ws::WsHandler`"), "{text}");
    assert!(text.contains("`cancel` is the envelope's reserved event, and a handler spells it"), "{text}");
}
