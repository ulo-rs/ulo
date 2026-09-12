//! A WebSocket server with no HTTP server beside it.
//!
//! `TungsteniteAdapter` implements `WebSocketAdapter` and nothing else: it
//! serves raw TCP, so a gateway reaches clients without an HTTP adapter being
//! registered at all. That is the case the same-port adapters cannot cover —
//! axum, poem and salvo carry WebSocket over their own HTTP router, and
//! declaring one to serve only WebSocket means running an HTTP server that
//! answers nothing.
//!
//! A gateway is matched here by the `port` in its attribute rather than by
//! path: this adapter takes the first path it is given and routes every
//! connection to it.
//!
//!     cargo run -p ulo-ws-tungstenite --example standalone_gateway
//!     websocat ws://127.0.0.1:3100
//!     {"event":"ping","data":{}}

use ulo::extractors::Payload;
use ulo::ws::{WsHandlerResult, WsMessage};
use ulo::*;
use ulo_macros::{module, new, subscriptions, websocket_gateway};
use ulo_ws_tungstenite::TungsteniteAdapter;

#[derive(serde::Deserialize)]
pub struct Echo {
    text: String,
}

#[websocket_gateway("/", port = 3100)]
pub struct EchoGateway {}

#[subscriptions]
impl EchoGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }

    #[subscribe_message("echo")]
    async fn echo(&self, Payload(echo): Payload<Echo>) -> WsHandlerResult {
        Ok(WsMessage::text(echo.text).into())
    }
}

#[module(providers: [EchoGateway])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut app = UloFactory::create(AppModule).await?;

    // No `use_http_adapter` call: this application serves WebSocket only.
    app.use_websocket_adapter(TungsteniteAdapter::new())?;

    println!("websocket on ws://127.0.0.1:3100");
    app.start().await?;
    Ok(())
}
