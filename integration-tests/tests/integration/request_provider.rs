//! `Request` injects into a controller without appearing in the module's
//! `providers:` list.
//!
//! It is registered by the framework rather than the author, so the thing that
//! breaks is the registration itself, and it breaks as an unresolved dependency
//! at startup.
use crate::common::TestServer;
use ulo::{Body as UloBody, Request, controller, get, module, routes};

#[controller("/test")]
pub struct TestController {
    #[inject]
    request: Request,
}

#[routes]
impl TestController {
    #[get("/info")]
    fn get_info(&self) -> UloBody {
        let method = self.request.method();
        let uri = self.request.uri();
        UloBody::text(format!("Method: {}, URI: {}", method, uri))
    }

    #[get("/headers")]
    fn get_headers(&self) -> UloBody {
        let content_type = self.request.header("content-type").unwrap_or("not found");
        UloBody::text(format!("Content-Type: {}", content_type))
    }
}

#[module(controllers: [TestController], providers: [])]
impl TestModule {}

#[tokio_localset_test::localset_test]
async fn request_auto_injected_without_providers_entry() {
    let server = TestServer::start(TestModule).await;

    let resp = server
        .client()
        .get(server.url("/test/info"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Method: GET"));
    assert!(body.contains("URI: /test/info"));

    let resp = server
        .client()
        .get(server.url("/test/headers"))
        .header("Content-Type", "application/json")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert!(
        resp.text()
            .await
            .unwrap()
            .contains("Content-Type: application/json")
    );
}
