//! Nothing handled the call, and that is a `Unrouted` event.
//!
//! An RPC pattern no controller claims used to be refused at the dispatcher
//! with no context built, so no error handler was consulted — the one call an
//! operator most wants to hear about was the only one nothing could claim. A
//! WebSocket event no handler subscribes to reached the chain already, but as a
//! bare `WsError`, so a catcher had to match a transport type rather than the
//! condition.
//!
//! Unclaimed, both render exactly what they rendered before.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use ulo::UloFactory;
use ulo::context::RpcContext;
use ulo::errors::Unrouted;
use ulo::rpc::{RpcData, RpcHandlerOutput, RpcHandlerResult};
use ulo::ws::WsContext;
use ulo::ws::{WsHandlerResult, WsMessage};
use ulo::{Error, catch, module};
use ulo_macros::{
    controller, message_pattern, new, patterns, subscribe_message, subscriptions, websocket_gateway,
};

use ulo::context::HandlerContext;

use crate::common::TestServer;

/// An RPC pattern no controller claims reached nothing that could have
/// declared metadata, so `metadata()` is `None`. Contrast the WebSocket
/// handler below, where the event did reach a gateway.
#[catch(Unrouted)]
async fn rpc_unrouted(err: &Unrouted, ctx: &RpcContext) -> RpcData {
    RpcData::from_serialize(&serde_json::json!({
        "missing": err.target,
        "metadata_is_none": ctx.metadata().is_none(),
    }))
    .unwrap()
}

/// A WebSocket event nothing subscribes to still arrived at a gateway, so the
/// gateway's impl-block declaration is the answer and `metadata()` is `Some` —
/// empty here because this gateway declares nothing, and the gateway's entries
/// where it does. `None` would mean nothing was reached at all, which is the
/// RPC case above and not this one.
#[catch(Unrouted)]
async fn ws_unrouted(err: &Unrouted, ctx: &WsContext) -> WsMessage {
    WsMessage::text(format!(
        "missing:{}:metadata_none={}",
        err.target,
        ctx.metadata().is_none()
    ))
}

// ── RPC ────────────────────────────────────────────────────────────────────

#[controller]
pub struct SomethingController {}

#[patterns]
impl SomethingController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("routed.echo")]
    async fn echo(&self) -> RpcHandlerResult {
        Ok(RpcHandlerOutput::Single(RpcData::text("routed")))
    }
}

#[module(controllers: [SomethingController])]
impl UnroutedRpcModule {}

async fn boot_rpc<F>(configure: F) -> u16
where
    F: FnOnce(&mut UloFactory) + Send + 'static,
{
    let (port_tx, port_rx) = tokio::sync::oneshot::channel::<u16>();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut factory = UloFactory::new();
        configure(&mut factory);
        let mut app = factory.create_with(UnroutedRpcModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = port_tx.send(bound.rpc.expect("rpc must bind").port());
        app.run().await;
    });
    tokio::task::spawn_local(async move { local.await });
    port_rx.await.expect("RPC server failed to bind")
}

async fn call(port: u16, pattern: &str) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    let stream = tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
        .await
        .unwrap();
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut frame = serde_json::json!({"pattern": pattern, "data": {}, "id": "1"}).to_string();
    frame.push('\n');
    writer.write_all(frame.as_bytes()).await.unwrap();

    let mut line = String::new();
    tokio::time::timeout(Duration::from_secs(2), reader.read_line(&mut line))
        .await
        .expect("a reply must arrive")
        .expect("the connection must stay readable");
    serde_json::from_str(&line).expect("the reply must be JSON")
}

/// A handler can claim the miss, which means the chain was reached.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unrouted_rpc_pattern_is_claimable() {
    let port = boot_rpc(|f| {
        f.use_global_rpc_error_handler(Arc::new(rpc_unrouted));
    })
    .await;
    let reply = call(port, "nobody.claims.this").await;

    assert_eq!(
        reply["response"]["missing"], "nobody.claims.this",
        "reply: {reply}"
    );
    assert_eq!(
        reply["response"]["metadata_is_none"], true,
        "a call no handler claimed declares nothing, so metadata() is None \
         rather than an empty Metadata: {reply}"
    );
}

/// Unclaimed, the caller sees the frame it always saw.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unclaimed_rpc_miss_renders_as_before() {
    let port = boot_rpc(|_| {}).await;
    let reply = call(port, "nobody.claims.this").await;

    assert_eq!(reply["err"]["status"], "not_found", "reply: {reply}");
}

// ── WebSocket ──────────────────────────────────────────────────────────────

#[websocket_gateway("/ws-unrouted")]
pub struct SomethingGateway {}

#[subscriptions]
impl SomethingGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("routed")]
    async fn routed(&self) -> WsHandlerResult {
        Ok(WsMessage::text("routed").into())
    }
}

#[module(providers: [SomethingGateway])]
impl UnroutedWsModule {}

async fn ask_ws(factory: UloFactory, event: &str) -> String {
    let server = TestServer::start_with(factory, UnroutedWsModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-unrouted", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        format!(r#"{{"event":"{event}"}}"#).into(),
    ))
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("a reply must arrive")
        .expect("the socket stays open")
        .expect("the frame arrives")
        .into_text()
        .unwrap()
        .to_string()
}

#[serial]
#[tokio_localset_test::localset_test]
async fn an_unrouted_ws_event_is_claimable() {
    let mut factory = UloFactory::new();
    factory.use_global_ws_error_handler(Arc::new(ws_unrouted));

    assert_eq!(
        ask_ws(factory, "nobody-claims-this").await,
        "missing:nobody-claims-this:metadata_none=false",
        "an unrouted WS event still reached a gateway, so it inherits that \
         gateway's declaration rather than answering None"
    );
}

/// Unclaimed, the envelope is the one it always was.
#[serial]
#[tokio_localset_test::localset_test]
async fn an_unclaimed_ws_miss_renders_as_before() {
    let reply = ask_ws(UloFactory::new(), "nobody-claims-this").await;
    let reply: serde_json::Value = serde_json::from_str(&reply).expect("an error envelope");

    assert_eq!(reply["status"], "error");
    assert_eq!(reply["kind"], "NotFound", "envelope: {reply}");
}
