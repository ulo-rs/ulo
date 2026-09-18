//! The wire format `#[sse]` and `Sse::new(stream)` produce: the headers a client
//! needs to keep the connection open, and the `data:`/`event:`/`id:` framing of
//! each event.
//!
//! A browser's `EventSource` rejects a stream that frames events wrongly, and
//! the framing is assembled from `SseEvent` fields rather than written by the
//! handler, so the bytes on the socket are the contract. Both stream item types
//! are covered — infallible and per-event fallible — along with multiline data,
//! which is the case that must be re-prefixed rather than sent as one line.
//!
//! `#[sse]` accepts three shapes, and the routes below spell each one differently: an `impl
//! Stream`, a boxed stream behind an item alias, and a `Result` whose `Err` fails the call before
//! any event is written. A handler's answer is read from its type, not from how the type is
//! written.
use std::pin::Pin;
use std::time::Duration;

use crate::common::TestServer;
use futures_util::{StreamExt, stream};
use tokio::sync::broadcast;
use ulo::http::{HttpResponse, Sse, SseEvent};
use ulo::sse;
use ulo::{controller, get, http::extract::Bytes, module, post, routes};
use ulo_macros::{injectable, new};

// ── Service ──────────────────────────────────────────────────────────────────

#[injectable]
pub struct EventsService {
    tx: broadcast::Sender<String>,
}
impl EventsService {
    #[new]
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(64);
        Self { tx }
    }

    pub fn emit(&self, data: String) {
        let _ = self.tx.send(data);
    }

    pub fn subscribe(
        &self,
    ) -> Pin<Box<dyn futures_util::stream::Stream<Item = SseEvent> + Send + Sync + 'static>> {
        let rx = self.tx.subscribe();
        Box::pin(stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(data) => return Some((SseEvent::data(data), rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        }))
    }
}

/// Fails an `#[sse]` handler's setup, before the first event.
#[derive(ulo::Error, Debug)]
#[error_kind(Forbidden)]
struct NoSubscription;

impl std::fmt::Display for NoSubscription {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("no subscription")
    }
}

impl std::error::Error for NoSubscription {}

/// A fallible item behind an alias: the item type is not spelled `Result` at the handler.
type AliasedEvent = Result<SseEvent, std::io::Error>;

// ── Controller ───────────────────────────────────────────────────────────────

#[controller("/sse")]
pub struct SseController {
    #[inject]
    events: EventsService,
}

#[routes]
impl SseController {
    #[get("/basic")]
    async fn basic(&self) -> impl ulo::dispatch::IntoOutput<ulo::dispatch::Http> {
        Sse::new(stream::iter([
            SseEvent::data("hello"),
            SseEvent::data("world"),
        ]))
    }

    #[get("/fields")]
    async fn fields(&self) -> impl ulo::dispatch::IntoOutput<ulo::dispatch::Http> {
        Sse::new(stream::iter([SseEvent::data("payload")
            .event("update")
            .id("42")
            .retry_ms(3000)]))
    }

    #[get("/multiline")]
    async fn multiline(&self) -> impl ulo::dispatch::IntoOutput<ulo::dispatch::Http> {
        Sse::new(stream::iter([SseEvent::data("line1\nline2\nline3")]))
    }

    #[get("/fallible")]
    async fn fallible(&self) -> impl ulo::dispatch::IntoOutput<ulo::dispatch::Http> {
        Sse::new(stream::iter([Ok::<SseEvent, std::io::Error>(
            SseEvent::data("ok-event"),
        )]))
    }

    // Bounded to 2 events so the test connection closes after receiving them
    #[get("/live")]
    async fn live(&self) -> impl ulo::dispatch::IntoOutput<ulo::dispatch::Http> {
        Sse::new(self.events.subscribe().take(2))
    }

    // `use<>` because Rust 2024 has `impl Trait` capture `&self`'s lifetime by
    // default, and the stream a route returns has to outlive the borrow.
    #[sse("/attr-basic")]
    async fn attr_basic(&self) -> impl futures_util::Stream<Item = SseEvent> + use<> {
        stream::iter([SseEvent::data("hello"), SseEvent::data("world")])
    }

    #[sse("/attr-fallible")]
    async fn attr_fallible(
        &self,
    ) -> impl futures_util::Stream<Item = Result<SseEvent, std::io::Error>> + use<> {
        stream::iter([Ok(SseEvent::data("ok-event"))])
    }

    // A boxed stream whose item is an alias: neither is spelled `impl Stream<Item = Result<..>>`.
    #[sse("/attr-boxed")]
    async fn attr_boxed(&self) -> Pin<Box<dyn futures_util::Stream<Item = AliasedEvent> + Send>> {
        Box::pin(stream::iter([Ok(SseEvent::data("boxed-event"))]))
    }

    #[sse("/attr-setup-ok")]
    async fn attr_setup_ok(
        &self,
    ) -> Result<impl futures_util::Stream<Item = SseEvent> + use<>, NoSubscription> {
        Ok(stream::iter([SseEvent::data("subscribed")]))
    }

    // The stream type is named rather than opaque: this handler never builds an `Ok`, and
    // `impl Trait` has nothing to infer from.
    #[sse("/attr-setup-err")]
    async fn attr_setup_err(&self) -> Result<stream::Empty<SseEvent>, NoSubscription> {
        Err(NoSubscription)
    }

    #[post("/emit")]
    async fn emit_event(
        &self,
        Bytes(data): Bytes,
    ) -> impl ulo::dispatch::IntoOutput<ulo::dispatch::Http> {
        self.events
            .emit(String::from_utf8_lossy(&data).into_owned());
        HttpResponse::no_content().build()
    }
}

#[module(controllers: [SseController], providers: [EventsService])]
impl SseModule {}

// ── Tests ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_sse_headers() {
    let server = TestServer::start(SseModule).await;
    let resp = server
        .client()
        .get(server.url("/sse/basic"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        ct.contains("text/event-stream"),
        "expected text/event-stream, got {ct}"
    );
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .unwrap()
            .to_str()
            .unwrap(),
        "no-cache"
    );
    assert_eq!(
        resp.headers()
            .get("x-accel-buffering")
            .unwrap()
            .to_str()
            .unwrap(),
        "no"
    );
}

#[tokio::test]
async fn test_sse_basic_wire_format() {
    let server = TestServer::start(SseModule).await;
    let body = server
        .client()
        .get(server.url("/sse/basic"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "data: hello\n\ndata: world\n\n");
}

#[tokio::test]
async fn test_sse_event_fields() {
    let server = TestServer::start(SseModule).await;
    let body = server
        .client()
        .get(server.url("/sse/fields"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(body.contains("id: 42\n"), "missing id line");
    assert!(body.contains("event: update\n"), "missing event line");
    assert!(body.contains("retry: 3000\n"), "missing retry line");
    assert!(body.contains("data: payload\n"), "missing data line");
}

#[tokio::test]
async fn test_sse_multiline_data() {
    let server = TestServer::start(SseModule).await;
    let body = server
        .client()
        .get(server.url("/sse/multiline"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // Multi-line data must be split into separate "data:" lines per SSE spec
    assert_eq!(body, "data: line1\ndata: line2\ndata: line3\n\n");
}

#[tokio::test]
async fn test_sse_fallible_stream() {
    let server = TestServer::start(SseModule).await;
    let body = server
        .client()
        .get(server.url("/sse/fallible"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "data: ok-event\n\n");
}

#[tokio::test]
async fn test_sse_broadcaster_delivers_to_subscriber() {
    let server = TestServer::start(SseModule).await;

    let live_url = server.url("/sse/live");
    let emit_url = server.url("/sse/emit");
    let client = server.client().clone();

    // Subscribe first, then emit concurrently. The /live handler takes 2 events
    // and closes, so the text() call completes once both events arrive.
    let (body, _, _) = tokio::join!(
        async {
            client
                .get(&live_url)
                .send()
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        },
        async {
            // Allow the subscription request to reach the server before emitting
            tokio::time::sleep(Duration::from_millis(100)).await;
            client.post(&emit_url).body("hello").send().await.unwrap();
        },
        async {
            tokio::time::sleep(Duration::from_millis(150)).await;
            client.post(&emit_url).body("world").send().await.unwrap();
        },
    );

    assert_eq!(body, "data: hello\n\ndata: world\n\n");
}

#[tokio::test]
async fn test_sse_attr_macro_infallible() {
    let server = TestServer::start(SseModule).await;
    let resp = server
        .client()
        .get(server.url("/sse/attr-basic"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/event-stream"));
    let body = resp.text().await.unwrap();
    assert_eq!(body, "data: hello\n\ndata: world\n\n");
}

#[tokio::test]
async fn test_sse_attr_macro_fallible() {
    let server = TestServer::start(SseModule).await;
    let body = server
        .client()
        .get(server.url("/sse/attr-fallible"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert_eq!(body, "data: ok-event\n\n");
}

/// A boxed stream behind an item alias streams like any other fallible stream.
#[tokio::test]
async fn an_aliased_boxed_stream_streams() {
    let server = TestServer::start(SseModule).await;
    let resp = server
        .client()
        .get(server.url("/sse/attr-boxed"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "data: boxed-event\n\n");
}

/// Setup that succeeds streams the events, with the headers and body of a bare stream.
#[tokio::test]
async fn fallible_setup_that_succeeds_streams() {
    let server = TestServer::start(SseModule).await;
    let resp = server
        .client()
        .get(server.url("/sse/attr-setup-ok"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers().get("content-type").unwrap(),
        "text/event-stream"
    );
    assert_eq!(resp.text().await.unwrap(), "data: subscribed\n\n");
}

/// Setup that fails answers the error, not an empty event stream: the `Err` reaches the transport's
/// renderer and carries the domain error's own kind.
#[tokio::test]
async fn fallible_setup_that_fails_answers_the_error() {
    let server = TestServer::start(SseModule).await;
    let resp = server
        .client()
        .get(server.url("/sse/attr-setup-err"))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 403);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["statusCode"], 403);
    assert_eq!(body["message"], "no subscription");
}
