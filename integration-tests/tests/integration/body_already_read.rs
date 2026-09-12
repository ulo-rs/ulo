//! The runtime half of the one-body rule, for the extractor the compile-time
//! check cannot see.
//!
//! `CONSUMES` is what the handler macros sum to reject two body readers at
//! compile time. An extractor that reads through `take_body` and leaves
//! `CONSUMES` at its `false` default is not counted, so a handler may declare
//! two of them and compile. The second to run then finds the body gone.
//!
//! That path existed with no test. It is the escape hatch the trait documents —
//! a guard that reads the body works the same way — and the failure it produces
//! is the application's fault rather than the caller's, which is why it is
//! logged at error level as well as answered.

use ulo::context::HttpContext;
use ulo::extractors::{FromContext, take_body};
use ulo::{Body, controller, module, post, routes};

use crate::common::TestServer;

/// Reads the body and does not say so. The compile-time check counts
/// `CONSUMES`, and this leaves it at the default.
pub struct QuietBodyReader(pub usize);

impl FromContext<HttpContext> for QuietBodyReader {
    type Error = ulo::extractors::BodyAlreadyRead;

    // CONSUMES deliberately left at its `false` default.

    async fn extract(ctx: &HttpContext) -> Result<Self, Self::Error> {
        let req = take_body::<Self>(ctx)?;
        let (_, body) = req.into_parts();
        let bytes = body.collect().await.unwrap_or_default();
        Ok(QuietBodyReader(bytes.len()))
    }
}

#[controller("/quiet")]
pub struct QuietController {}

#[routes]
impl QuietController {
    /// Two uncounted body readers. This compiles, which is the point.
    #[post("/twice")]
    async fn twice(&self, first: QuietBodyReader, second: QuietBodyReader) -> Body {
        Body::text(format!("{}/{}", first.0, second.0))
    }

    #[post("/once")]
    async fn once(&self, only: QuietBodyReader) -> Body {
        Body::text(only.0.to_string())
    }
}

#[module(controllers: [QuietController])]
impl QuietModule {}

/// One uncounted reader is fine, which is what makes the two-reader case below
/// a second-read failure rather than the extractor being broken.
#[tokio_localset_test::localset_test]
async fn one_uncounted_reader_gets_the_body() {
    let server = TestServer::start(QuietModule).await;

    let resp = server
        .client()
        .post(server.url("/quiet/once"))
        .body("hello")
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "5");
}

/// The second reader finds the body gone, and the answer says which extractor
/// asked.
#[tokio_localset_test::localset_test]
async fn a_second_uncounted_reader_is_told_the_body_is_gone() {
    let server = TestServer::start(QuietModule).await;

    let resp = server
        .client()
        .post(server.url("/quiet/twice"))
        .body("hello")
        .send()
        .await
        .unwrap();

    assert_eq!(
        resp.status(),
        400,
        "a body read twice renders as an extraction failure"
    );
    let body: serde_json::Value = resp.json().await.expect("the answer is JSON");
    assert_eq!(body["error"], "Extraction failed");
    let details = body["details"].as_str().unwrap_or_default();
    assert!(
        details.contains("QuietBodyReader"),
        "the details name the extractor that asked second: {body}"
    );
}
