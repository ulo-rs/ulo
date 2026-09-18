//! An `HttpError` returned from a middleware renders with the status it names.
//!
//! Middleware runs before the error chain has a matched route to scope handlers
//! against, so the failure mode is a 500 that erases the status the middleware
//! chose. Both a custom status and a named kind are covered.
// Verifies that HttpError returned from Middleware::handle maps to the correct
// HTTP status code rather than collapsing to 500.
//
// Before the fix, Err(e) in the middleware chain was re-boxed as io::Error,
// losing type information, and always produced 500.

use crate::common::TestServer;
use ulo::async_trait;
use ulo::di::MiddlewareConsumer;
use ulo::http::Body;
use ulo::http::HttpError;
use ulo::http::middleware::{Middleware, MiddlewareResult, NextHandle};
use ulo::{controller, get, module, routes};
// ── Test 1: custom status code ────────────────────────────────────────────────

struct RejectWith(HttpError);

#[async_trait]
impl Middleware for RejectWith {
    async fn handle(&self, _next: NextHandle) -> MiddlewareResult {
        Err(Box::new(self.0.clone()))
    }
}

#[tokio::test]
async fn middleware_http_error_preserves_status() {
    #[controller("/")]
    pub struct PingController {}

    #[routes]
    impl PingController {
        #[get("/ping")]
        fn ping(&self) -> Body {
            Body::text("pong")
        }
    }

    #[module(controllers: [PingController])]
    impl TestModule {
        fn configure_middleware(&self, consumer: &mut MiddlewareConsumer) {
            consumer
                .apply(RejectWith(HttpError::custom(429, "rate limit exceeded")))
                .for_routes(vec!["/*"]);
        }
    }

    let server = TestServer::start(TestModule).await;
    let resp = server
        .client()
        .get(server.url("/ping"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        429,
        "HttpError status should be preserved, not collapsed to 500"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 429);
    assert_eq!(body["message"], "rate limit exceeded");
}

// ── Test 2: named variant (Unauthorized) ─────────────────────────────────────

#[tokio::test]
async fn middleware_http_error_unauthorized() {
    #[controller("/")]
    pub struct AuthController {}

    #[routes]
    impl AuthController {
        #[get("/secret")]
        fn secret(&self) -> Body {
            Body::text("secret")
        }
    }

    #[module(controllers: [AuthController])]
    impl AuthModule {
        fn configure_middleware(&self, consumer: &mut MiddlewareConsumer) {
            consumer
                .apply(RejectWith(HttpError::unauthorized("token expired")))
                .for_routes(vec!["/*"]);
        }
    }

    let server = TestServer::start(AuthModule).await;
    let resp = server
        .client()
        .get(server.url("/secret"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 401);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 401);
    assert_eq!(body["message"], "token expired");
}
