//! A refused connection tells the caller why.
//!
//! Connect guards run after the handshake — there is no HTTP status left to
//! refuse with — so the refusal is answered the way RFC 6455 provides for:
//! the canonical envelope as a text frame, then a close carrying the code for
//! that refusal. A browser reads both off its `close` event.
//!
//! Before this, the adapter dropped the socket and the caller could not tell
//! a refusal from a crash or a dead network.

#![allow(dead_code)]

use futures_util::{SinkExt, StreamExt};
use serial_test::serial;
use ulo::async_trait;
use ulo::enhancer::Guard;
use ulo::ws::WsContext;
use ulo::ws::{WsHandlerResult, WsMessage};
use ulo::{injectable, module};
use ulo_macros::{new, subscribe_message, subscriptions, websocket_gateway};

use crate::common::TestServer;

#[injectable]
pub struct DenyConnect {}

#[async_trait]
impl Guard<WsContext> for DenyConnect {
    async fn can_activate(&self, _ctx: &WsContext) -> bool {
        false
    }
}

#[injectable]
pub struct PanicOnConnect {}

#[async_trait]
impl Guard<WsContext> for PanicOnConnect {
    async fn can_activate(&self, _ctx: &WsContext) -> bool {
        panic!("connect guard kaboom");
    }
}

#[websocket_gateway("/ws-refused")]
pub struct RefusedGateway {}

#[subscriptions]
#[use_guards(DenyConnect)]
impl RefusedGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self) -> WsHandlerResult {
        Ok(WsMessage::text("unreachable").into())
    }
}

#[module(providers: [DenyConnect, RefusedGateway])]
impl RefusedConnectModule {}

#[websocket_gateway("/ws-connect-panic")]
pub struct PanicConnectGateway {}

#[subscriptions]
#[use_guards(PanicOnConnect)]
impl PanicConnectGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("echo")]
    async fn echo(&self) -> WsHandlerResult {
        Ok(WsMessage::text("unreachable").into())
    }
}

#[module(providers: [PanicOnConnect, PanicConnectGateway])]
impl PanicConnectModule {}

/// Reads the envelope frame and the close frame a refusal answers with.
async fn refusal_of(url: &str) -> (serde_json::Value, u16) {
    let (mut ws, _) = tokio_tungstenite::connect_async(url)
        .await
        .expect("the handshake completes — guards run after it");

    let envelope = ws
        .next()
        .await
        .expect("a frame must arrive")
        .expect("the frame is readable");
    let envelope: serde_json::Value =
        serde_json::from_str(envelope.to_text().expect("a text frame")).expect("an envelope");

    let close = loop {
        match ws.next().await {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => break frame,
            Some(Ok(_)) => continue,
            other => panic!("expected a close frame, got {other:?}"),
        }
    };
    let code = u16::from(close.expect("the close carries a frame").code);

    // Nothing is sent after the close.
    let _ = ws.close(None).await;
    (envelope, code)
}

#[serial]
#[tokio::test]
async fn a_refused_connection_is_told_the_policy() {
    let server = TestServer::start(RefusedConnectModule).await;
    let (envelope, code) = refusal_of(&format!("ws://127.0.0.1:{}/ws-refused", server.port)).await;

    assert_eq!(envelope["status"], "error");
    assert_eq!(envelope["kind"], "Unauthorized", "envelope: {envelope}");
    // 1008 Policy Violation, in place of the registry's 3000 Unauthorized.
    assert_eq!(code, 1008);
}

#[serial]
#[tokio::test]
async fn a_panicking_connect_guard_closes_as_a_server_fault() {
    let server = TestServer::start(PanicConnectModule).await;
    let (envelope, code) =
        refusal_of(&format!("ws://127.0.0.1:{}/ws-connect-panic", server.port)).await;

    // The panic keeps its type through the refusal, which is what separates a
    // server fault from a policy one on the wire.
    assert_eq!(envelope["kind"], "Internal", "envelope: {envelope}");
    assert!(
        envelope["message"]
            .as_str()
            .unwrap_or_default()
            .contains("connect guard kaboom"),
        "envelope: {envelope}",
    );
    assert_eq!(code, 1011);
}

// ── what a connect does not run ────────────────────────────────────────────

/// Records every enhancer that ran, so the absence of one is observable rather
/// than merely unasserted.
static CONNECT_RAN: std::sync::OnceLock<std::sync::Mutex<Vec<&'static str>>> =
    std::sync::OnceLock::new();

fn connect_ran() -> &'static std::sync::Mutex<Vec<&'static str>> {
    CONNECT_RAN.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

#[ulo::injectable]
pub struct RecordingGuard {}

#[ulo::async_trait]
impl ulo::enhancer::Guard<ulo::ws::WsContext> for RecordingGuard {
    async fn can_activate(&self, ctx: &ulo::ws::WsContext) -> bool {
        connect_ran()
            .lock()
            .unwrap()
            .push(if ctx.event() == "connect" {
                "guard:connect"
            } else {
                "guard:message"
            });
        true
    }
}

#[ulo::injectable]
pub struct RecordingInterceptor {}

#[ulo::async_trait]
impl ulo::enhancer::Interceptor<ulo::ws::WsContext, WsHandlerResult> for RecordingInterceptor {
    async fn intercept(
        &self,
        ctx: &ulo::ws::WsContext,
        next: Box<dyn ulo::enhancer::InterceptorNext<ulo::ws::WsContext, WsHandlerResult>>,
    ) -> WsHandlerResult {
        connect_ran()
            .lock()
            .unwrap()
            .push(if ctx.event() == "connect" {
                "interceptor:connect"
            } else {
                "interceptor:message"
            });
        next.run(ctx).await
    }
}

#[websocket_gateway("/ws-skips")]
pub struct SkipGateway {}

#[subscriptions]
#[use_guards(RecordingGuard)]
#[use_interceptors(RecordingInterceptor)]
impl SkipGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[module(providers: [SkipGateway, RecordingGuard, RecordingInterceptor])]
struct SkipModule;

/// A connect runs its guards and nothing else. An interceptor wraps a call and
/// its answer; a connection has neither, and refusing one is answered by not
/// opening it. The same enhancers do run on the message that follows, which is
/// what makes the absence a decision rather than a registration that failed.
#[serial]
#[tokio::test]
async fn a_connect_runs_guards_and_not_interceptors() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    connect_ran().lock().unwrap().clear();

    let server = TestServer::start(SkipModule).await;
    let url = format!("ws://127.0.0.1:{}/ws-skips", server.port);
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    assert_eq!(
        *connect_ran().lock().unwrap(),
        vec!["guard:connect"],
        "a connect consults its guards and no interceptor"
    );

    ws.send(Message::Text(r#"{"event":"ping"}"#.to_string().into()))
        .await
        .unwrap();
    let reply = ws.next().await.unwrap().unwrap();
    assert_eq!(reply.to_text().unwrap(), "pong");

    assert_eq!(
        *connect_ran().lock().unwrap(),
        vec!["guard:connect", "guard:message", "interceptor:message"],
        "the same interceptor does run for a message, so its absence above is \
         the connect path's decision rather than a registration that failed"
    );
}
