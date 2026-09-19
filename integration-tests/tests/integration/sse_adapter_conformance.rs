//! Which adapters can serve an SSE route, and what happens on one that cannot.
//!
//! An adapter that collects a response body before sending it cannot serve a stream that does not
//! end: it never finishes collecting, so no response head is written and the request hangs. A
//! bounded stream does arrive, whole, once it ends — which is what makes the mismatch worth
//! refusing rather than leaving to be discovered, since the route registers and looks served
//! either way. The refusal is raised where routes mount, at `use_http_adapter`.

use crate::common::TestServer;
use futures_util::stream;
use ulo::http::SseEvent;
use ulo::{UloFactory, controller, module, new, routes, sse};

#[controller("/feed")]
pub struct FeedController {}

#[routes]
impl FeedController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[sse("/events")]
    async fn events(&self) -> impl futures_util::Stream<Item = SseEvent> + use<> {
        stream::iter([SseEvent::data("one"), SseEvent::data("two")])
    }
}

#[module(controllers: [FeedController])]
impl FeedModule {}

async fn case_streams_the_events(adapter: impl ulo::http::HttpAdapter + 'static) {
    let server = TestServer::start_adapter(UloFactory::new(), FeedModule, adapter).await;

    let resp = server
        .client()
        .get(server.url("/feed/events"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert_eq!(resp.text().await.unwrap(), "data: one\n\ndata: two\n\n");
}

macro_rules! sse_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            #[tokio::test]
            async fn serves_an_sse_route() {
                super::case_streams_the_events($adapter).await;
            }
        }
    };
}

sse_suite!(axum, ulo_http_axum::AxumAdapter::new());
sse_suite!(poem, ulo_http_poem::PoemAdapter::new());
sse_suite!(salvo, ulo_http_salvo::SalvoAdapter::new());
sse_suite!(rocket, ulo_http_rocket::RocketAdapter::new());

/// Actix buffers a response body, so it is refused the route rather than given one it would
/// register and never answer.
///
/// The refusal is raised at `use_http_adapter`, where routes are mounted, rather than at `bind()`.
#[tokio::test]
async fn actix_refuses_an_sse_route() {
    let mut app = UloFactory::new().create_with(FeedModule).await.unwrap();

    let msg = match app.use_http_adapter(ulo_http_actix::ActixAdapter::new(), ("127.0.0.1", 0)) {
        Ok(_) => panic!("actix collects a response body and cannot serve an SSE route"),
        Err(e) => e.to_string(),
    };
    assert!(
        msg.contains("answers with a stream") && msg.contains("/feed/events"),
        "expected a refusal naming the route and the limitation, got: {msg}"
    );
}
