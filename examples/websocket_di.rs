// WebSocket example with DI container integration and enhancers
//
// This example demonstrates:
// 1. Automatic gateway discovery
// 2. Global guards and interceptors applied to all gateways
// 3. Full integration of WebSocket with ulo's DI system
// 4. Zero manual wiring - framework handles everything automatically

use ulo::context::WsContext;
use ulo::traits::{Guard, Interceptor, InterceptorNext};
use ulo::websocket::{BroadcastModule, BroadcastService};
use ulo::*;
use ulo_macros::{injectable, module, new, subscriptions, websocket_gateway};

#[injectable]
pub struct WsAuthGuard;

#[async_trait]
impl Guard<WsContext> for WsAuthGuard {
    async fn can_activate(&self, ctx: &WsContext) -> bool {
        println!("[WsAuthGuard] Checking authentication...");
        if let Some(token) = ctx.client().handshake.headers.get("x-auth-token") {
            println!("[WsAuthGuard] ✅ Auth token found: {}", token);
            return true;
        }
        println!("[WsAuthGuard] ❌ No auth token - connection rejected");
        false
    }
}

#[injectable]
pub struct WsLoggingInterceptor;

#[async_trait]
impl Interceptor<WsContext, WsHandlerResult> for WsLoggingInterceptor {
    async fn intercept(
        &self,
        ctx: &WsContext,
        next: Box<dyn InterceptorNext<WsContext, WsHandlerResult>>,
    ) -> WsHandlerResult {
        println!("[WsLoggingInterceptor] 📥 Incoming message");
        println!("  Client: {}", ctx.client().id);
        println!("  Event: {}", ctx.event());
        let answer = next.run(ctx).await;
        println!("[WsLoggingInterceptor] 📤 Message processed");
        answer
    }
}

#[websocket_gateway("/chat")]
pub struct ChatGateway {
    broadcast: BroadcastService,
}
#[subscriptions]
#[use_guards(WsAuthGuard)]
#[use_interceptors(WsLoggingInterceptor)]
impl ChatGateway {
    #[new]
    pub fn new(broadcast: BroadcastService) -> Self {
        Self { broadcast }
    }

    #[subscribe_message("message")]
    async fn handle_message(
        &self,
        client: ulo::WsClient,
        message: ulo::WsMessage,
    ) -> ulo::WsHandlerResult {
        let text = message
            .as_text()
            .ok_or_else(|| ulo::WsError::InvalidMessage("Expected text message".into()))?;

        println!("[ChatGateway] Received from {}: {}", client.id, text);

        let response = format!("Broadcast: {}", text);
        self.broadcast
            .to_all()
            .send_event("message", &response)
            .await?;

        Ok(ulo::WsHandlerOutput::Empty)
    }

    #[subscribe_message("ping")]
    async fn handle_ping(
        &self,
        _client: ulo::WsClient,
        _message: ulo::WsMessage,
    ) -> ulo::WsHandlerResult {
        Ok(ulo::WsMessage::text("pong").into())
    }
}

#[module(
    imports: [BroadcastModule::new()],
    providers: [
        ChatGateway,
        WsAuthGuard,
        WsLoggingInterceptor
    ]
)]
struct ChatModule;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 WebSocket DI Example - Automatic Gateway Discovery\n");
    println!("This example demonstrates:");
    println!("  • Zero manual wiring - framework auto-discovers gateways");
    println!("  • BroadcastService injected via DI");
    println!("  • Global guards and interceptors applied to all gateways\n");

    println!("WebSocket endpoint: ws://127.0.0.1:8080/chat\n");

    println!("Test WITHOUT auth token (will be rejected by guard):");
    println!(r#"  websocat ws://127.0.0.1:8080/chat"#);
    println!();

    println!("Test WITH auth token (will succeed):");
    println!(r#"  websocat -H='X-Auth-Token: secret123' ws://127.0.0.1:8080/chat"#);
    println!(r#"  Send: {{"event": "message", "data": "Hello"}}"#);
    println!();

    println!("Alternative header syntax:");
    println!(r#"  websocat --header='X-Auth-Token: secret123' ws://127.0.0.1:8080/chat"#);
    println!();

    let factory = UloFactory::new();
    let mut app = factory.create_with(ChatModule).await?;

    app.use_http_adapter(ulo_http_axum::AxumAdapter::new(), ("127.0.0.1", 8080))
        .unwrap();

    println!("✅ Server ready - guards and interceptors active!\n");

    app.start().await?;
    Ok(())
}
