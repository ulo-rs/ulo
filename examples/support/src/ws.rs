//! What the WebSocket example tests share: serving an app whose gateways are `port = own`, and a
//! client socket that reads and writes the JSON message envelope. Every wait is bounded by
//! [`WAIT`] of real time, so a frame that never arrives fails the test instead of hanging it.

use std::net::SocketAddr;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::AppBuilder;
use ulo::testing::ScriptedTimer;
use ulo_runtime_tokio::Tokio;
use ulo_ws_hyper::Server;

use crate::{Served, serve};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// The longest any one wait on a socket lasts.
pub const WAIT: Duration = Duration::from_secs(10);

/// Wires, connects and binds `app` on the tokio runtime and a WebSocket server on port 0, then
/// serves it. Panics on a startup failure.
pub async fn start_ws(app: AppBuilder) -> Served {
    start_ws_with(app.runtime(Tokio::current())).await
}

/// As [`start_ws`], under `clock`: a drain window of the app passes when the test advances the
/// clock and not before.
pub async fn start_ws_on(app: AppBuilder, clock: ScriptedTimer) -> Served {
    start_ws_with(app.runtime(Tokio::current()).timer(clock)).await
}

async fn start_ws_with(app: AppBuilder) -> Served {
    let app = app.wire().expect("the app wires").connect().await.expect("the app connects");
    serve(app.bind(Server::new("127.0.0.1:0")).listen().await.expect("the app listens"))
}

/// An upgrade to `path` on `addr` carrying `headers`, answered 101.
pub async fn connect(addr: SocketAddr, path: &str, headers: &[(&str, &str)]) -> Socket {
    let mut request = format!("ws://{addr}{path}").into_client_request().expect("a WebSocket request");
    for (name, value) in headers {
        request.headers_mut().insert(
            tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
            value.parse().expect("a header value"),
        );
    }
    let (socket, _) = tokio::time::timeout(WAIT, tokio_tungstenite::connect_async(request))
        .await
        .unwrap_or_else(|_| panic!("the upgrade to {path} did not finish within {WAIT:?}"))
        .unwrap_or_else(|error| panic!("the upgrade to {path} failed: {error}"));
    socket
}

/// Sends `message` as one text frame.
pub async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::text(message.to_string())).await.unwrap_or_else(|error| panic!("sending {message} failed: {error}"));
}

/// The next text frame as JSON, skipping pings and pongs.
pub async fn next(socket: &mut Socket) -> Value {
    match next_message(socket).await {
        Message::Text(text) => serde_json::from_str(&text).unwrap_or_else(|error| panic!("a frame is not JSON: {error}: {text}")),
        other => panic!("expected a text frame, got {other:?}"),
    }
}

/// The code and reason of the close frame that comes next; a data frame before it fails the test.
pub async fn close_frame(socket: &mut Socket) -> Option<(u16, String)> {
    match next_message(socket).await {
        Message::Close(frame) => frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())),
        other => panic!("expected a close frame, got {other:?}"),
    }
}

/// Closes the client's side with 1000, or answers a close the server sent, and reads until the
/// server ends the connection.
pub async fn hang_up(socket: Socket) {
    hang_up_with(socket, 1000, "").await;
}

/// As [`hang_up`], with `code` and `reason` on the close frame.
pub async fn hang_up_with(mut socket: Socket, code: u16, reason: &str) {
    let frame = CloseFrame { code: CloseCode::from(code), reason: reason.to_owned().into() };
    let _ = socket.close(Some(frame)).await;
    tokio::time::timeout(WAIT, async { while let Some(Ok(_)) = socket.next().await {} })
        .await
        .unwrap_or_else(|_| panic!("the server did not end the connection within {WAIT:?}"));
}

async fn next_message(socket: &mut Socket) -> Message {
    tokio::time::timeout(WAIT, async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                Some(Ok(message)) => return message,
                other => panic!("expected a frame, got {other:?}"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no frame arrived within {WAIT:?}"))
}
