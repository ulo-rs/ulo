//! What the axum↔ulo boundary does with a real socket under it: route
//! resolution, buffered and streaming request bodies, and WebSocket upgrade on
//! both the shared port and a separate one.
//!
//! Route matching and the pre-routing chain are proved for every adapter at
//! once in `integration-tests` (the four `*_conformance` suites). What is left
//! to this file is what only axum does: it is the reference adapter for body
//! streaming, and the only one carrying both WebSocket port modes through its
//! own router.

use futures_util::{SinkExt, StreamExt};
use ulo::UloFactory;
use ulo::extractors::{BodyStream, Bytes, Path, Query};
use ulo::*;
use ulo_http_axum::AxumAdapter;
use ulo_macros::{module, new, subscriptions, websocket_gateway};

#[derive(Debug, serde::Deserialize)]
struct SearchParams {
    q: String,
}

#[controller("/api")]
pub struct ApiController;

#[routes]
impl ApiController {
    #[get("/hello")]
    fn hello(&self) -> Body {
        Body::text("hello")
    }

    #[get("/users/{id}")]
    fn user(&self, id: Path<String>) -> Body {
        Body::text(format!("user {}", id.0))
    }

    #[get("/search")]
    fn search(&self, q: Query<SearchParams>) -> Body {
        Body::text(format!("q={}", q.0.q))
    }

    #[post("/echo")]
    async fn echo(&self, body: Bytes) -> Body {
        Body::text(format!("echo:{}", body.0.len()))
    }

    #[post("/count")]
    async fn count(&self, body: BodyStream) -> Body {
        let mut total = 0u64;
        let mut chunks = 0u32;
        let mut s = Box::pin(body.into_stream());
        while let Some(chunk) = s.next().await {
            if let Ok(b) = chunk {
                total += b.len() as u64;
                chunks += 1;
            }
        }
        Body::text(format!("count:{total} chunks:{chunks}"))
    }
}

#[websocket_gateway("/ws")]
pub struct EchoGateway {}

#[subscriptions]
impl EchoGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("pong").into())
    }
}

#[websocket_gateway("/separate", port = 0)]
pub struct SeparateGateway {}

#[subscriptions]
impl SeparateGateway {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[subscribe_message("ping")]
    async fn ping(&self, _client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        Ok(WsMessage::text("separate-pong").into())
    }
}

#[module(controllers: [ApiController], providers: [EchoGateway])]
impl HttpOnlyModule {}

#[module(controllers: [ApiController], providers: [EchoGateway, SeparateGateway])]
impl FullModule {}

struct Bound {
    http_addr: std::net::SocketAddr,
    ws_addr: Option<std::net::SocketAddr>,
}

/// Port 0 throughout: a fixed port makes two of these tests a race against each
/// other and against whatever else holds the port on the machine.
async fn start(module: impl ulo::ModuleMetadata + 'static, with_ws_adapter: bool) -> Bound {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = UloFactory::create(module).await.unwrap();
        app.use_http_adapter(AxumAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        if with_ws_adapter {
            app.use_websocket_adapter(AxumAdapter::new()).unwrap();
        }
        let bound = app.bind().await.unwrap();
        let http = bound.http.expect("HTTP not bound");
        let ws = bound.websocket.first().copied();
        let _ = tx.send(Bound {
            http_addr: http,
            ws_addr: ws,
        });
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });
    rx.await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn http_get_path_param_query_route_through_axum() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(HttpOnlyModule, false).await;
            let base = format!("http://{}", bound.http_addr);
            let client = reqwest::Client::new();

            let r = client
                .get(format!("{base}/api/hello"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "hello");

            let r = client
                .get(format!("{base}/api/users/42"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "user 42");

            let r = client
                .get(format!("{base}/api/search?q=axum"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "q=axum");

            let r = client
                .get(format!("{base}/api/missing"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 404);
        })
        .await;
}

/// A megabyte arriving in more than one chunk is the observable difference
/// between streaming and the buffering the actix adapter does: byte count alone
/// cannot tell the two apart.
#[tokio::test(flavor = "current_thread")]
async fn http_post_buffered_and_streaming_bodies() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(HttpOnlyModule, false).await;
            let base = format!("http://{}", bound.http_addr);
            let client = reqwest::Client::new();

            let r = client
                .post(format!("{base}/api/echo"))
                .body("hello world")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "echo:11");

            let payload = vec![0u8; 1024 * 1024];
            let r = client
                .post(format!("{base}/api/count"))
                .body(payload)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            let body = r.text().await.unwrap();
            assert!(
                body.starts_with("count:1048576 "),
                "the handler must see every byte, got: {body}"
            );
            let chunks: u32 = body
                .rsplit_once("chunks:")
                .expect("the handler reports a chunk count")
                .1
                .parse()
                .expect("the chunk count is a number");
            assert!(
                chunks > 1,
                "a megabyte arrived in {chunks} chunk(s); the body was buffered, not streamed"
            );
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_same_port_upgrade_and_echo() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(HttpOnlyModule, false).await;
            let url = format!("ws://{}/ws", bound.http_addr);

            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"event":"ping"}"#.to_string().into(),
            ))
            .await
            .unwrap();

            let msg = ws.next().await.unwrap().unwrap();
            assert_eq!(msg.to_text().unwrap(), "pong");
        })
        .await;
}

#[tokio::test(flavor = "current_thread")]
async fn ws_separate_port_upgrade_and_echo() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let bound = start(FullModule, true).await;
            let ws_addr = bound.ws_addr.expect("WS adapter not bound");
            let url = format!("ws://{ws_addr}/separate");

            let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
            ws.send(tokio_tungstenite::tungstenite::Message::Text(
                r#"{"event":"ping"}"#.to_string().into(),
            ))
            .await
            .unwrap();

            let msg = ws.next().await.unwrap().unwrap();
            assert_eq!(msg.to_text().unwrap(), "separate-pong");
        })
        .await;
}
