//! `UpgradeHead::query` decodes the handshake's query string as HTTP's `Query<T>` decodes one:
//! read by a connect guard, by a session factory and by a message handler, percent-decoded with
//! `+` as a space, a repeated name filling a `Vec`, and a missing or unparsable field named.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode as WireCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, BoxError, Dep, Guard, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers, injectable};
use ulo_transport::ExtractError;
use ulo_ws::{ConnectCx, Session, UpgradeHead, WsConnect};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const FRAME_WAIT: Duration = Duration::from_secs(5);

#[derive(Deserialize)]
struct Ticket {
    token: String,
}

/// Admits a handshake whose query carries `token=open sesame`, however it is encoded.
#[injectable]
struct TicketGuard;

impl Guard<WsConnect> for TicketGuard {
    async fn can_activate(&self, cx: &ConnectCx) -> Result<bool, BoxError> {
        Ok(cx.head().query::<Ticket>().is_ok_and(|ticket| ticket.token == "open sesame"))
    }
}

#[derive(Deserialize)]
struct Join {
    room: String,
    #[serde(default)]
    tag: Vec<String>,
}

/// A field the handler requires, which the `strict` message reads.
#[derive(Deserialize)]
struct Strict {
    #[expect(dead_code, reason = "read only to decide whether the query decodes")]
    seat: u32,
}

#[controller]
struct Lobby;

#[handlers]
#[ulo_ws::gateway(
    path = "/lobby",
    port = own,
    connect_guards(TicketGuard),
    session_with = |head: Dep<UpgradeHead>| head.query::<Join>().ok()
)]
impl Lobby {
    #[ulo_ws::message("joined")]
    fn joined(&self, join: Session<Option<Join>>) -> Value {
        match &*join {
            Some(join) => json!({ "room": join.room, "tags": join.tag }),
            None => Value::Null,
        }
    }

    /// The outcome of reading the query as `Strict`, as `ok`, `missing <param>` or
    /// `malformed <param>`.
    #[ulo_ws::message("strict")]
    fn strict(&self, head: Dep<UpgradeHead>) -> String {
        match head.query::<Strict>() {
            Ok(_) => "ok".to_owned(),
            Err(ExtractError::Missing { param }) => format!("missing {param}"),
            Err(ExtractError::Malformed { param, .. }) => format!("malformed {param}"),
            Err(other) => format!("other {other}"),
        }
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.provide::<TicketGuard>();
        m.controller::<Lobby>();
    }
}

struct Running {
    addr: SocketAddr,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

impl Running {
    async fn start() -> Running {
        let app: App<Serving> = App::builder(Root)
            .runtime(ulo_runtime_tokio::Tokio::current())
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
        let [bound] = app.addresses().try_into().unwrap_or_else(|other| panic!("expected one bound address, got {other:?}"));
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        Running { addr: bound.addr, handle, serving }
    }

    async fn connect(&self, query: &str) -> Socket {
        let (socket, response) = tokio_tungstenite::connect_async(format!("ws://{}/lobby?{query}", self.addr))
            .await
            .unwrap_or_else(|error| panic!("the handshake for `?{query}` failed: {error}"));
        assert_eq!(response.status(), 101, "`?{query}`");
        socket
    }

    async fn stop(self) {
        let _ = self.handle.close(Signal::new("test")).await;
        let _ = self.serving.await;
    }
}

async fn ask(socket: &mut Socket, event: &str) -> Value {
    let message = json!({ "event": event, "id": 1 });
    socket.send(Message::text(message.to_string())).await.unwrap_or_else(|error| panic!("sending `{event}` failed: {error}"));
    match tokio::time::timeout(FRAME_WAIT, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => {
            let reply: Value = serde_json::from_str(&text).unwrap_or_else(|error| panic!("non-JSON reply {text:?}: {error}"));
            reply["data"].clone()
        }
        other => panic!("expected a reply to `{event}`, got {other:?}"),
    }
}

async fn closed_with(socket: &mut Socket) -> Option<WireCode> {
    match tokio::time::timeout(FRAME_WAIT, socket.next()).await {
        Ok(Some(Ok(Message::Close(frame)))) => frame.map(|frame| frame.code),
        other => panic!("expected the refused connection to close, got {other:?}"),
    }
}

#[tokio::test]
async fn a_guard_and_a_session_factory_read_the_query_decoded() {
    let app = Running::start().await;

    let mut socket = app.connect("room=caf%C3%A9+one&tag=a&token=open%20sesame&tag=b%26c").await;
    assert_eq!(ask(&mut socket, "joined").await, json!({ "room": "café one", "tags": ["a", "b&c"] }));
    let _ = socket.close(None).await;

    let mut socket = app.connect("token=open+sesame&room=hall").await;
    assert_eq!(ask(&mut socket, "joined").await, json!({ "room": "hall", "tags": [] }));
    let _ = socket.close(None).await;

    app.stop().await;
}

#[tokio::test]
async fn a_guard_reading_a_token_the_query_lacks_or_encodes_otherwise_refuses() {
    let app = Running::start().await;
    for query in ["room=hall", "token=open%2Bsesame", "token=opensesame"] {
        let mut socket = app.connect(query).await;
        assert_eq!(closed_with(&mut socket).await, Some(WireCode::from(3003)), "`?{query}`");
        let _ = socket.close(None).await;
    }
    app.stop().await;
}

#[tokio::test]
async fn a_missing_or_unparsable_field_is_named() {
    let app = Running::start().await;
    for (query, expected) in [
        ("token=open+sesame&seat=12", "ok"),
        ("token=open+sesame", "missing seat"),
        ("token=open+sesame&seat=twelve", "malformed seat"),
        ("token=open+sesame&seat=1&seat=2", "malformed seat"),
    ] {
        let mut socket = app.connect(query).await;
        assert_eq!(ask(&mut socket, "strict").await, json!(expected), "`?{query}`");
        let _ = socket.close(None).await;
    }
    app.stop().await;
}
