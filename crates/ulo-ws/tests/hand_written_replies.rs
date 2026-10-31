//! `WsCx::reply` and `WsCx::reply_stream` answer as an attribute-written handler's return value
//! does: `()` answers no data, an `Err` item ends a stream with the error envelope, and the reply
//! is encoded by the gateway's codec. Each hand-written handler's name differs from its event, so
//! the event is read from `HandlerSpec::route`.

mod support;

use futures_util::stream::{self, Stream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use ulo::scope::Auto;
use ulo::{
    BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver, injectable, routes,
};
use ulo_ws::{
    CallError, Codec, ErrorKind, FromCall, GatewayConfig, GatewayHooks, GatewaySettings, Payload, Reply, Ws, WsCx, WsHandler,
    WsModule,
};

use support::{Running, hang_up, next_json, next_message, send_json};

fn taken() -> CallError {
    CallError::new(ErrorKind::Conflict, "taken")
}

#[injectable]
struct Macro;

#[routes]
#[ulo_ws::gateway(path = "/m")]
impl Macro {
    #[ulo_ws::message("unit")]
    fn unit(&self) {}

    #[ulo_ws::message("failing")]
    fn failing(&self) -> impl Stream<Item = Result<u32, CallError>> + use<> {
        stream::iter([Ok(1), Err(taken()), Ok(2)])
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

impl GatewayConfig for Hand {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/h")
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        GatewayHooks::<Self>::new().mount(m);
    }
}

async fn unit(cx: WsCx) -> Result<Reply, BoxError> {
    Ok(cx.reply(&())?)
}

async fn failing(cx: WsCx) -> Result<Reply, BoxError> {
    Ok(cx.reply_stream(stream::iter([Ok::<u32, CallError>(1), Err(taken()), Ok(2)])))
}

impl Controller for Hand {
    fn mount(m: &mut Mount<'_>) {
        m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
        m.handler(HandlerSpec::<Ws, _>::new("answer_nothing", WsHandler::new(unit)).route("unit"));
        m.handler(HandlerSpec::<Ws, _>::new("stream_then_fail", WsHandler::new(failing)).route("failing"));
    }
}

struct Packed;

impl Construct for Packed {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Packed)
    }
}

impl GatewayConfig for Packed {
    fn settings() -> GatewaySettings {
        GatewaySettings::at("/packed").codec(Codec::MsgPack)
    }

    fn mount_gateway(m: &mut Mount<'_>) {
        GatewayHooks::<Self>::new().mount(m);
    }
}

async fn echo(cx: WsCx) -> Result<Reply, BoxError> {
    let Payload(text) = <Payload<String> as FromCall<Ws>>::from_call(&cx).await?;
    Ok(cx.reply(&format!("echo:{text}"))?)
}

impl Controller for Packed {
    fn mount(m: &mut Mount<'_>) {
        m.once::<Self>(<Self as GatewayConfig>::mount_gateway);
        m.handler(HandlerSpec::<Ws, _>::new("echo", WsHandler::new(echo)).route("echo"));
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<Macro>();
        m.controller::<Hand>();
        m.controller::<Packed>();
    }
}

/// The frames that answer a `unit` message then a `failing` one, up to the error envelope.
async fn answers(app: &Running, path: &str) -> Vec<Value> {
    let mut socket = app.connect(path, &[]).await;
    send_json(&mut socket, &json!({ "event": "unit", "id": 1 })).await;
    send_json(&mut socket, &json!({ "event": "failing", "id": 2 })).await;
    let mut frames = Vec::new();
    loop {
        let frame = next_json(&mut socket).await;
        let done = frame.get("error").is_some();
        frames.push(frame);
        if done {
            break;
        }
    }
    hang_up(socket).await;
    frames
}

#[tokio::test]
async fn a_hand_written_reply_answers_as_a_returned_value_does() {
    let app = Running::start(Root, ulo_http_hyper::Server::new("127.0.0.1:0")).await;
    let attributed = answers(&app, "/m").await;
    let written = answers(&app, "/h").await;
    assert_eq!(written, attributed);
    assert_eq!(written.first(), Some(&json!({ "id": 1, "complete": true })), "`()` answers no data: {written:?}");
    assert_eq!(written.last().and_then(|frame| frame.pointer("/error/kind")), Some(&json!("conflict")), "{written:?}");

    let mut socket = app.connect("/packed", &[]).await;
    let message = rmp_serde::to_vec_named(&json!({ "event": "echo", "id": 1, "data": "hi" })).expect("a MessagePack envelope");
    socket.send(Message::binary(message)).await.expect("the message is sent");
    let Message::Binary(bytes) = next_message(&mut socket).await else { panic!("a MessagePack gateway answers in binary frames") };
    let answer: Value = rmp_serde::from_slice(&bytes).expect("a MessagePack answer");
    assert_eq!(answer, json!({ "id": 1, "data": "echo:hi" }));
    let _ = socket.next();
    hang_up(socket).await;
    app.stop().await;
}
