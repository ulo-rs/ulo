//! Every adapter serves an SSE route. What an adapter that could not is told instead.
//!
//! An adapter that collects a response body before sending it cannot serve a stream that does not
//! end: it never finishes collecting, so no response head is written and the request hangs. A
//! bounded stream does arrive, whole, once it ends — which is what makes the mismatch worth
//! refusing rather than leaving to be discovered, since the route registers and looks served
//! either way. `HttpAdapter::streams_responses` is how an adapter says which it is, and the
//! refusal is raised where routes mount, at `use_http_adapter`. None of the five answers `false`;
//! the one that exercises the refusal is written here.

use std::sync::Arc;
use std::time::Duration;

use crate::common::TestServer;
use futures_util::stream;
use ulo::http::{
    HttpAdapter, HttpLifecycleHandle, HttpMethod, HttpResponse, RequestHandler, ServeContext, Sse,
    SseEvent,
};
use ulo::spi::{AdapterResult, BindTarget};
use ulo::{UloFactory, async_trait, controller, get, module, new, routes, sse};

#[controller("/feed")]
pub struct FeedController {}

#[routes]
impl FeedController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[sse("/events")]
    async fn events(&self) -> impl futures_util::Stream<Item = SseEvent> {
        stream::iter([SseEvent::data("one"), SseEvent::data("two")])
    }

    /// Never ends, which is the shape a live feed has and the one a collecting adapter cannot
    /// answer at all. The gap between events is what lets a reader see them arrive separately.
    #[sse("/ticks")]
    async fn ticks(&self) -> impl futures_util::Stream<Item = SseEvent> {
        stream::unfold((), |()| async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Some((SseEvent::data("tick"), ()))
        })
    }
}

#[module(controllers: [FeedController])]
impl FeedModule {}

/// The same answer written as a plain `#[get]`, the form `Route::streams` cannot see. Its own
/// module keeps it out of the five servers the suite above starts.
#[controller("/undeclared")]
pub struct UndeclaredController {}

#[routes]
impl UndeclaredController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[get("/events")]
    async fn events(&self) -> HttpResponse {
        Sse::new(stream::iter([SseEvent::data("one"), SseEvent::data("two")])).into()
    }
}

#[module(controllers: [UndeclaredController])]
impl UndeclaredModule {}

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

/// The head is written before the body is known, so a stream with no end still answers.
///
/// A bounded stream cannot show this: an adapter that collects one answers too, only late. Here
/// there is nothing to collect, so a collecting adapter writes no head and the request hangs —
/// which is why every wait below is bounded. The reader takes two events and leaves, which is all
/// an endless feed ever offers.
async fn case_answers_an_endless_stream(adapter: impl ulo::http::HttpAdapter + 'static) {
    let server = TestServer::start_adapter(UloFactory::new(), FeedModule, adapter).await;

    let mut resp = tokio::time::timeout(
        Duration::from_secs(5),
        server.client().get(server.url("/feed/ticks")).send(),
    )
    .await
    .expect("no response head while the body was still open")
    .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );

    let mut seen = String::new();
    while seen.matches("\n\n").count() < 2 {
        let chunk = tokio::time::timeout(Duration::from_secs(5), resp.chunk())
            .await
            .expect("the stream stalled")
            .unwrap()
            .expect("the stream ended, and this one does not");
        seen.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(
        seen.starts_with("data: tick\n\ndata: tick\n\n"),
        "expected two events, got: {seen:?}"
    );
}

macro_rules! sse_suite {
    ($adapter_mod:ident, $adapter:expr) => {
        mod $adapter_mod {
            #[tokio::test]
            async fn serves_an_sse_route() {
                super::case_streams_the_events($adapter).await;
            }

            #[tokio::test]
            async fn answers_an_endless_sse_route() {
                super::case_answers_an_endless_stream($adapter).await;
            }
        }
    };
}

sse_suite!(axum, ulo_http_axum::AxumAdapter::new());
sse_suite!(poem, ulo_http_poem::PoemAdapter::new());
sse_suite!(salvo, ulo_http_salvo::SalvoAdapter::new());
sse_suite!(rocket, ulo_http_rocket::RocketAdapter::new());
sse_suite!(actix, ulo_http_actix::ActixAdapter::new());

/// An adapter that collects. Being refused is all that is asked of it, and registration is as
/// far as it goes.
struct CollectingAdapter;

#[async_trait]
impl HttpAdapter for CollectingAdapter {
    fn streams_responses(&self) -> bool {
        false
    }

    fn register_route(
        &mut self,
        _method: HttpMethod,
        _path: &str,
        _handler: Arc<dyn RequestHandler>,
    ) -> AdapterResult {
        Ok(())
    }

    async fn into_lifecycle(
        self: Box<Self>,
        _target: BindTarget,
        _ctx: ServeContext,
    ) -> AdapterResult<HttpLifecycleHandle> {
        unreachable!("the route is refused before a socket is asked for")
    }
}

/// An adapter that collects is refused the route rather than given one it would register and
/// never answer.
///
/// The refusal is raised at `use_http_adapter`, where routes are mounted, rather than at `bind()`.
#[tokio::test]
async fn an_adapter_that_collects_is_refused_an_sse_route() {
    let mut app = UloFactory::new().create_with(FeedModule).await.unwrap();

    let msg = match app.use_http_adapter(CollectingAdapter, ("127.0.0.1", 0)) {
        Ok(_) => panic!("an adapter that collects cannot serve an SSE route"),
        Err(e) => e.to_string(),
    };
    // Either SSE route may be reached first — mount order is not a promise — so what is asserted
    // is that the refusal names the one it stopped at.
    assert!(
        msg.contains("answers with a stream") && msg.contains("/feed/"),
        "expected a refusal naming the route and the limitation, got: {msg}"
    );
}

/// A route that declares nothing streams on actix too. What decides is the body the adapter is
/// handed, not what the route declared.
#[tokio::test]
async fn an_undeclared_streaming_body_is_served_on_actix() {
    let server = TestServer::start_adapter(
        UloFactory::new(),
        UndeclaredModule,
        ulo_http_actix::ActixAdapter::new(),
    )
    .await;

    let resp = server
        .client()
        .get(server.url("/undeclared/events"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "data: one\n\ndata: two\n\n");
}
