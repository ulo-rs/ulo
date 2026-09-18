use std::convert::Infallible;
use std::fmt::Write;

use bytes::Bytes;
use futures::Stream;
use futures::StreamExt;

use super::{Body, HttpResponse};
use crate::dispatch::{Answer, Http, IntoOutput};

/// A single Server-Sent Event.
///
/// Build one with [`SseEvent::data`], then chain optional fields:
///
/// ```rust,ignore
/// SseEvent::data("hello")
///     .event("greet")
///     .id("1")
///     .retry_ms(3000)
/// ```
pub struct SseEvent {
    data: String,
    id: Option<String>,
    event: Option<String>,
    retry: Option<u64>,
}

impl SseEvent {
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            data: data.into(),
            id: None,
            event: None,
            retry: None,
        }
    }

    /// Sets the event's `id` field. The browser sends it back as `Last-Event-ID` on reconnect.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Sets the event type. Clients listen with `es.addEventListener("name", ...)`.
    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.event = Some(name.into());
        self
    }

    /// Suggests how long (in milliseconds) the browser should wait before reconnecting.
    pub fn retry_ms(mut self, ms: u64) -> Self {
        self.retry = Some(ms);
        self
    }

    fn encode(self) -> Bytes {
        let mut buf = String::new();
        if let Some(id) = self.id {
            writeln!(buf, "id: {id}").unwrap();
        }
        if let Some(event) = self.event {
            writeln!(buf, "event: {event}").unwrap();
        }
        if let Some(ms) = self.retry {
            writeln!(buf, "retry: {ms}").unwrap();
        }
        // SSE spec: multi-line data must be split into separate "data:" lines
        for line in self.data.lines() {
            writeln!(buf, "data: {line}").unwrap();
        }
        if self.data.is_empty() {
            buf.push_str("data: \n");
        }
        buf.push('\n'); // blank line terminates the event
        Bytes::from(buf)
    }
}

/// An item an SSE stream may yield.
///
/// Implemented for [`SseEvent`] and for `Result<SseEvent, E>`. A stream of either is an SSE body,
/// so a handler chooses per-event fallibility by its item type and by nothing else.
pub trait SseItem {
    /// What a failed event carries — [`Infallible`] for a bare [`SseEvent`].
    type Error: Into<Box<dyn std::error::Error + Send + Sync>> + 'static;

    fn into_result(self) -> Result<SseEvent, Self::Error>;
}

impl SseItem for SseEvent {
    type Error = Infallible;

    fn into_result(self) -> Result<SseEvent, Infallible> {
        Ok(self)
    }
}

impl<E> SseItem for Result<SseEvent, E>
where
    E: Into<Box<dyn std::error::Error + Send + Sync>> + 'static,
{
    type Error = E;

    fn into_result(self) -> Result<SseEvent, E> {
        self
    }
}

/// An SSE response. Wraps a stream of events and sets the required headers.
///
/// # Example
///
/// ```rust,ignore
/// use futures::stream;
/// use ulo::http::{Sse, SseEvent};
///
/// #[get("/events")]
/// async fn events(&self) -> impl IntoOutput<Http> {
///     Sse::new(stream::iter([
///         SseEvent::data("hello").event("greet"),
///         SseEvent::data("world").id("2"),
///     ]))
/// }
/// ```
///
/// A stream of `Result<SseEvent, E>` goes through the same constructor.
pub struct Sse<S>(S);

impl<S> Sse<S>
where
    S: Stream,
    S::Item: SseItem,
{
    pub fn new(stream: S) -> Self {
        Self(stream)
    }
}

impl<S> IntoOutput<Http> for Sse<S>
where
    S: Stream + Send + 'static,
    S::Item: SseItem,
{
    fn into_output(self) -> Answer<Http> {
        let encoded = self.0.map(|item| item.into_result().map(SseEvent::encode));
        Ok(HttpResponse {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/event-stream".into()),
                ("Cache-Control".into(), "no-cache".into()),
                // Tells nginx/caddy not to buffer the response before forwarding
                ("X-Accel-Buffering".into(), "no".into()),
            ],
            body: Some(Body::stream(encoded)),
        })
    }
}
