//! What a caller receives when an extractor cannot produce its value, and what
//! the application can do about it.
//!
//! The status was asserted in several places and the body in none, so a change
//! that answered a bare 400, or renamed either field, passed every test. The
//! body is what a client parses.
//!
//! The second claim here is a negative: an extraction failure does not reach
//! the error chain. It is written by the handler wrapper before any enhancer
//! sees a value, so `#[catch]` cannot reshape it — unlike a guard rejection or
//! a handler's own error, which both do reach the chain.

use std::sync::Arc;

use serde::Deserialize;
use ulo::async_trait;
use ulo::context::HttpContext;
use ulo::extractors::{Json, Query};
use ulo::http_helpers::HttpResponse;
use ulo::traits_helpers::{ChainError, ErrorHandler};
use ulo::{Body as UloBody, UloFactory, controller, get, module, post, routes};

use crate::common::TestServer;

#[derive(Deserialize)]
pub struct NeedsName {
    #[allow(dead_code)]
    name: String,
}

#[controller("/x")]
pub struct XController {}

#[routes]
impl XController {
    #[get("/q")]
    fn q(&self, _q: Query<NeedsName>) -> UloBody {
        UloBody::text("unreachable")
    }

    #[post("/j")]
    async fn j(&self, _j: Json<NeedsName>) -> UloBody {
        UloBody::text("unreachable")
    }
}

#[module(controllers: [XController])]
impl XModule {}

/// The envelope, field by field.
#[tokio_localset_test::localset_test]
async fn an_extraction_failure_names_itself_and_says_why() {
    let server = TestServer::start(XModule).await;

    for (label, resp) in [
        (
            "query",
            server
                .client()
                .get(server.url("/x/q"))
                .send()
                .await
                .unwrap(),
        ),
        (
            "json",
            server
                .client()
                .post(server.url("/x/j"))
                .json(&serde_json::json!({ "wrong": 1 }))
                .send()
                .await
                .unwrap(),
        ),
    ] {
        assert_eq!(resp.status(), 400, "{label}: a bad input is the caller's");
        let body: serde_json::Value = resp.json().await.expect("the answer is JSON");
        assert_eq!(
            body["error"], "Extraction failed",
            "{label}: the envelope names the failure: {body}"
        );
        assert!(
            body["details"].is_string() && !body["details"].as_str().unwrap().is_empty(),
            "{label}: details say which extractor and why: {body}"
        );
    }
}

/// Claims every error it is offered, so reaching the chain at all is visible.
struct ClaimEverything;

#[async_trait]
impl ErrorHandler<HttpContext, HttpResponse> for ClaimEverything {
    async fn handle_error(&self, _e: ChainError<'_>, _ctx: &HttpContext) -> Option<HttpResponse> {
        let mut resp = HttpResponse::new();
        resp.status = 599;
        resp.body = Some(UloBody::text("claimed-by-chain"));
        Some(resp)
    }
}

/// An extraction failure is written before any enhancer holds a value, so the
/// chain never sees it. A handler that claims everything still does not claim
/// this one.
#[tokio_localset_test::localset_test]
async fn an_extraction_failure_does_not_reach_the_error_chain() {
    let mut factory = UloFactory::new();
    factory.use_global_http_error_handler(Arc::new(ClaimEverything));
    let server = TestServer::start_with(factory, XModule).await;

    let resp = server
        .client()
        .get(server.url("/x/q"))
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "a chain handler claiming everything would answer 599"
    );
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["error"], "Extraction failed");
}
