//! What the actix↔ulo boundary does with a real socket under it, and what it
//! does differently from every other adapter: bodies are collected in both
//! directions before either side sees them.
//!
//! Route matching and the pre-routing chain are proved for every adapter at
//! once in `integration-tests` (the four `*_conformance` suites). What is left
//! to this file is the buffering, which no conformance case can express —
//! passing it is what the other adapters do, and actix must fail it.

use futures_util::StreamExt;
use ulo::UloFactory;
use ulo::extractors::{BodyStream, Bytes, Path};
use ulo::*;
use ulo_http_actix::ActixAdapter;
use ulo_macros::module;

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

    #[post("/echo")]
    async fn echo(&self, body: Bytes) -> Body {
        Body::text(format!("echo:{}", body.0.len()))
    }

    /// A `BodyStream` handler compiles and runs on actix; what reaches it is
    /// the collected body behind a one-item stream.
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

#[module(controllers: [ApiController])]
impl HttpOnlyModule {}

/// Port 0: a fixed port makes two of these tests a race against each other and
/// against whatever else holds the port on the machine.
async fn start() -> std::net::SocketAddr {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let local = tokio::task::LocalSet::new();
    local.spawn_local(async move {
        let mut app = UloFactory::create(HttpOnlyModule).await.unwrap();
        app.use_http_adapter(ActixAdapter::new(), ("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.http.expect("HTTP not bound"));
        app.run().await;
    });
    tokio::task::spawn_local(async move {
        local.await;
    });
    rx.await.unwrap()
}

#[tokio::test(flavor = "current_thread")]
async fn http_get_path_param_route_through_actix() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let addr = start().await;
            let base = format!("http://{addr}");
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
        })
        .await;
}

/// actix-web's `PayloadConfig` default, which the adapter never raises.
const PAYLOAD_LIMIT: usize = 262_144;

/// The adapter wraps the collected payload in a single-item stream, so the body
/// arrives whole. Asserting the byte count alone would pass on every adapter;
/// the chunk count is what distinguishes this one.
#[tokio::test(flavor = "current_thread")]
async fn a_streaming_handler_receives_the_body_already_collected() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let addr = start().await;
            let base = format!("http://{addr}");
            let client = reqwest::Client::new();

            let r = client
                .post(format!("{base}/api/echo"))
                .body("hello world")
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(r.text().await.unwrap(), "echo:11");

            let payload = vec![0u8; PAYLOAD_LIMIT];
            let r = client
                .post(format!("{base}/api/count"))
                .body(payload)
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
            assert_eq!(
                r.text().await.unwrap(),
                format!("count:{PAYLOAD_LIMIT} chunks:1"),
                "actix collects the payload before dispatch; more than one chunk \
                 means it grew a streaming path and the adapter table is now wrong"
            );
        })
        .await;
}

/// A request one byte over the limit is refused with 413 before any handler
/// runs — the ceiling every other adapter lacks, and the reason a handler
/// written against axum can stop working when the adapter is swapped.
///
/// ulo exposes no knob for it: raising the ceiling means registering an actix
/// `PayloadConfig`, which the adapter does not surface.
#[tokio::test(flavor = "current_thread")]
async fn a_payload_over_the_actix_limit_is_refused() {
    let local = tokio::task::LocalSet::new();
    local
        .run_until(async {
            let addr = start().await;
            let client = reqwest::Client::new();

            let r = client
                .post(format!("http://{addr}/api/count"))
                .body(vec![0u8; PAYLOAD_LIMIT + 1])
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 413);
        })
        .await;
}
