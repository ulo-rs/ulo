//! The connection hooks an attributed gateway implements, found by probing its type: each
//! `DisconnectReason` `OnDisconnect` receives, the session still readable there, the refused
//! connections that never reach it, and `AfterInit` on the standalone server.

mod support;

use futures_util::SinkExt;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::Frame as WireFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use ulo::{BoxError, Dep, Guard, Module, ModuleDef, ModuleIdentity, controller, handlers};
use serde::Deserialize;
use ulo_http::{StatusCode, WithHttpStatus};
use ulo_transport::{CallError, ErrorKind};
use ulo_ws::{
    AfterInit, CloseCode, ConnectCx, ConnectRefused, Connection, DisconnectReason, GatewayRef, OnConnect, OnDisconnect, Payload,
    UpgradeHead, WsConnect, WsCx,
};

use support::{Record, Running, close_frame, hang_up, hang_up_with, next_json, send_json, within};

const NAME: &str = "x-name";

/// What `kick` closes the connection with.
#[derive(Deserialize)]
struct Kick {
    code: u16,
    reason: String,
}

/// The visitor a connect guard refuses, and the one `on_connect` refuses with its own code.
const GUARD_REFUSES: &str = "mallory";
const HOOK_REFUSES: &str = "eve";

/// The visitor a connect guard fails with a `Forbidden` error that names an HTTP status of its
/// own, which a WebSocket refusal does not read.
const TEAPOT: &str = "teapot";

/// The connection's session: the name its upgrade carried.
struct Visitor {
    name: String,
}

/// Each `on_disconnect`, as (the session's name, the reason).
#[derive(Clone)]
struct Departures(Record<(String, DisconnectReason)>);

/// Each `after_init`, as (path, namespace).
#[derive(Clone)]
struct Inits(Record<(String, Option<String>)>);

struct NotMallory;

impl Guard<WsConnect> for NotMallory {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        let name = cx.head().headers().get(NAME);
        if name.is_some_and(|name| name == TEAPOT) {
            return Err(CallError::new(ErrorKind::Forbidden, "no teapots").with_http_status(StatusCode::IM_A_TEAPOT).into());
        }
        Ok(name.is_none_or(|name| name != GUARD_REFUSES))
    }
}

#[controller]
struct Hooked {
    departures: Dep<Departures>,
    inits: Dep<Inits>,
}

#[handlers]
#[ulo_ws::gateway(
    path = "/hooked",
    namespace = "hooks",
    port = own,
    connect_guards(value = NotMallory),
    session_with = |head: Dep<UpgradeHead>| Visitor {
        name: head.headers().get(NAME).and_then(|name| name.to_str().ok()).unwrap_or("anonymous").to_owned(),
    },
)]
impl Hooked {
    /// Closes the connection from the server with the code and reason the message names. A code
    /// or reason no Close frame carries is refused where it is written, and the refusal's text is
    /// the answer; the answer to a close that was queued is never written.
    #[ulo_ws::message("kick")]
    async fn kick(&self, kick: Payload<Kick>, cx: WsCx) -> String {
        let code = match CloseCode::new(kick.0.code) {
            Ok(code) => code,
            Err(refused) => return refused.to_string(),
        };
        match cx.conn().close(code, &kick.0.reason).await {
            Ok(()) => "closing".to_owned(),
            Err(refused) => refused.to_string(),
        }
    }

    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }

    /// Fails with a `Conflict` that names an HTTP status of its own.
    #[ulo_ws::message("teapot")]
    fn teapot(&self) -> Result<String, CallError> {
        Err(CallError::new(ErrorKind::Conflict, "the pot is taken").with_http_status(StatusCode::IM_A_TEAPOT))
    }
}

impl OnConnect for Hooked {
    async fn on_connect(&self, cx: &ConnectCx) -> Result<(), ConnectRefused> {
        if cx.head().headers().get(NAME).is_some_and(|name| name == HOOK_REFUSES) {
            let code = CloseCode::new(4000).unwrap_or_else(|error| panic!("4000 is sendable: {error}"));
            return Err(ConnectRefused::code(code, "not you").unwrap_or_else(|error| panic!("the reason fits a Close frame: {error}")));
        }
        Ok(())
    }
}

impl OnDisconnect for Hooked {
    async fn on_disconnect(&self, conn: &Connection, why: DisconnectReason) {
        let name = conn.session().get::<Visitor>().map_or_else(|| "no session".to_owned(), |visitor| visitor.name.clone());
        self.departures.0.push((name, why));
    }
}

impl AfterInit for Hooked {
    async fn after_init(&self, gw: GatewayRef) {
        self.inits.0.push((gw.path().to_owned(), gw.namespace().map(str::to_owned)));
    }
}

struct Root {
    departures: Departures,
    inits: Inits,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.departures.clone());
        m.value(self.inits.clone());
        m.controller::<Hooked>();
    }
}

struct Started {
    app: Running,
    departures: Departures,
    inits: Inits,
}

async fn start() -> Started {
    let departures = Departures(Record::new());
    let inits = Inits(Record::new());
    let root = Root { departures: departures.clone(), inits: inits.clone() };
    let app = Running::start(root, ulo_ws_hyper::Server::new("127.0.0.1:0")).await;
    Started { app, departures, inits }
}

/// A connection as `name` whose connect phase has finished: no message is read before it.
async fn visit(app: &Running, name: &str) -> support::Socket {
    let mut socket = app.connect("/hooked", &[(NAME, name)]).await;
    send_json(&mut socket, &json!({ "event": "echo", "id": 0, "data": "ready" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 0, "data": "ready" }));
    socket
}

async fn departure(departures: &Departures) -> (String, DisconnectReason) {
    let all = departures.0.at_least(1, "`on_disconnect`").await;
    match all.as_slice() {
        [one] => one.clone(),
        more => panic!("expected one departure, got {more:?}"),
    }
}

#[tokio::test]
async fn a_close_the_client_sends_is_client_close_with_its_code_and_reason() {
    let Started { app, departures, .. } = start().await;
    let socket = visit(&app, "ada").await;
    hang_up_with(socket, 4001, "bye").await;
    let expected = ("ada".to_owned(), DisconnectReason::ClientClose { code: 4001, reason: "bye".to_owned() });
    assert_eq!(departure(&departures).await, expected);
    app.stop().await;
}

#[tokio::test]
async fn a_close_the_gateway_sends_is_server_close_with_its_code() {
    let Started { app, departures, .. } = start().await;
    let mut socket = visit(&app, "ada").await;
    send_json(&mut socket, &json!({ "event": "kick", "id": 1, "data": { "code": 4002, "reason": "kicked" } })).await;
    assert_eq!(close_frame(&mut socket).await, Some((4002, "kicked".to_owned())));
    hang_up(socket).await;
    assert_eq!(departure(&departures).await, ("ada".to_owned(), DisconnectReason::ServerClose { code: 4002 }));
    app.stop().await;
}

/// `Connection::close` refuses a reason over 123 bytes with nothing queued, so the connection stays
/// open and answers the next message: no other Close frame is written in its place.
#[tokio::test]
async fn a_close_reason_no_frame_may_carry_is_refused_and_the_connection_stays_open() {
    let Started { app, departures, .. } = start().await;
    let mut socket = visit(&app, "ada").await;
    let long = "x".repeat(124);
    send_json(&mut socket, &json!({ "event": "kick", "id": 1, "data": { "code": 4002, "reason": long } })).await;
    let refused = json!({ "id": 1, "data": "a close reason is at most 123 bytes of UTF-8, and this one is 124" });
    assert_eq!(next_json(&mut socket).await, refused);
    send_json(&mut socket, &json!({ "event": "echo", "id": 2, "data": "still open" })).await;
    assert_eq!(next_json(&mut socket).await, json!({ "id": 2, "data": "still open" }));
    hang_up(socket).await;
    assert_eq!(departure(&departures).await, ("ada".to_owned(), DisconnectReason::ClientClose { code: 1000, reason: String::new() }));
    app.stop().await;
}

#[tokio::test]
async fn a_text_frame_that_is_not_utf8_is_a_protocol_error_closed_with_1007() {
    let Started { app, departures, .. } = start().await;
    let mut socket = visit(&app, "ada").await;
    let frame = WireFrame::message(vec![0xff, 0xfe, 0xfd], OpCode::Data(Data::Text), true);
    socket.send(Message::Frame(frame)).await.unwrap_or_else(|error| panic!("sending the frame failed: {error}"));
    assert_eq!(close_frame(&mut socket).await.map(|(code, _)| code), Some(1007));
    hang_up(socket).await;
    assert_eq!(departure(&departures).await, ("ada".to_owned(), DisconnectReason::ProtocolError));
    app.stop().await;
}

#[tokio::test]
async fn a_connection_dropped_without_a_close_frame_is_lost() {
    let Started { app, departures, .. } = start().await;
    let socket = visit(&app, "ada").await;
    drop(socket);
    assert_eq!(departure(&departures).await, ("ada".to_owned(), DisconnectReason::Lost));
    app.stop().await;
}

#[tokio::test]
async fn the_drain_is_drain_and_on_disconnect_still_runs_and_reads_the_session() {
    let Started { app, departures, .. } = start().await;
    let mut socket = visit(&app, "ada").await;
    let stopping = tokio::spawn(app.stop());
    assert_eq!(close_frame(&mut socket).await, Some((1001, ulo_transport::SHUTDOWN_MESSAGE.to_owned())));
    hang_up(socket).await;
    within("the app's close", stopping).await.unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(departures.0.snapshot(), vec![("ada".to_owned(), DisconnectReason::Drain)]);
}

#[tokio::test]
async fn a_connection_the_connect_phase_refused_never_reaches_on_disconnect() {
    let Started { app, departures, .. } = start().await;
    for (name, code) in [(GUARD_REFUSES, 3003), (HOOK_REFUSES, 4000)] {
        let mut socket = app.connect("/hooked", &[(NAME, name)]).await;
        assert_eq!(close_frame(&mut socket).await.map(|(code, _)| code), Some(code), "{name}");
        hang_up(socket).await;
    }
    let socket = visit(&app, "ada").await;
    hang_up(socket).await;
    // The drain waits for every connection's task, so nothing is written to the record after it.
    app.stop().await;
    let expected = vec![("ada".to_owned(), DisconnectReason::ClientClose { code: 1000, reason: String::new() })];
    assert_eq!(departures.0.snapshot(), expected);
}

/// An error's HTTP status is HTTP's alone: a connect guard's `Forbidden` closes with 3003 and a
/// message handler's `Conflict` answers the envelope's `conflict`, whatever status it names.
#[tokio::test]
async fn an_error_s_http_status_changes_no_websocket_rendering() {
    let Started { app, .. } = start().await;
    let mut refused = app.connect("/hooked", &[(NAME, TEAPOT)]).await;
    assert_eq!(close_frame(&mut refused).await, Some((3003, "no teapots".to_owned())));
    hang_up(refused).await;
    let mut socket = visit(&app, "ada").await;
    send_json(&mut socket, &json!({ "event": "teapot", "id": 1 })).await;
    let error = json!({ "kind": "conflict", "message": "the pot is taken", "details": [] });
    assert_eq!(next_json(&mut socket).await, json!({ "id": 1, "error": error }));
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test]
async fn after_init_runs_once_after_listen_with_the_gateway_s_path_and_namespace() {
    let Started { app, inits, .. } = start().await;
    let calls = inits.0.at_least(1, "`AfterInit`").await;
    assert_eq!(calls, vec![("/hooked".to_owned(), Some("hooks".to_owned()))]);
    app.stop().await;
    assert_eq!(inits.0.snapshot().len(), 1, "`AfterInit` ran more than once");
}
