use std::borrow::Cow;
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
///
/// A value that the wire cannot carry is omitted rather than sent altered, and the omission is
/// logged at `warn`. See [`SseEvent::id`] and [`SseEvent::event`].
pub struct SseEvent {
    /// `None` is no `data:` line at all, which a reader dispatches nothing for. `Some("")` is a
    /// `data:` line carrying nothing, which it dispatches. The spec distinguishes them and so
    /// must this.
    data: Option<String>,
    id: Option<String>,
    event: Option<String>,
    retry: Option<u64>,
    comment: Option<String>,
}

impl SseEvent {
    pub fn data(data: impl Into<String>) -> Self {
        Self {
            data: Some(data.into()),
            id: None,
            event: None,
            retry: None,
            comment: None,
        }
    }

    /// An event whose data is `value` serialized as JSON.
    ///
    /// The same as [`SseEvent::data`] with the string written out. Fails only where `serde_json`
    /// does: a map key that is not a string, number or bool, or a type whose `Serialize` returns
    /// an error.
    ///
    /// ```rust,ignore
    /// SseEvent::json(&Reading { celsius: 21.5 })?.event("reading")
    /// ```
    pub fn json<T: serde::Serialize + ?Sized>(value: &T) -> Result<Self, serde_json::Error> {
        Ok(Self::data(serde_json::to_string(value)?))
    }

    /// A comment-only event: the `: text` line clients ignore.
    ///
    /// Carries no data, so a client dispatches nothing for it. That is what makes it the keepalive
    /// — it holds the connection open through an intermediary's idle timeout without delivering an
    /// event.
    ///
    /// # Holding an idle stream open
    ///
    /// Nothing on [`Sse`] or `SseEvent` builds the tick stream for you: a keepalive needs a clock,
    /// and this crate depends on no runtime. The application has one, and interleaving is the
    /// whole of it:
    ///
    /// ```rust,ignore
    /// use futures::stream;
    /// use std::time::Duration;
    ///
    /// let ticks = stream::unfold((), |_| async {
    ///     tokio::time::sleep(Duration::from_secs(15)).await;
    ///     Some((SseEvent::comment("keep-alive"), ()))
    /// });
    ///
    /// Sse::new(stream::select(events, ticks))
    /// ```
    ///
    /// A tick stream that never ends makes the response never end, which is what an SSE route
    /// usually wants; bound it if the route should finish on its own.
    pub fn comment(text: impl Into<String>) -> Self {
        let mut event = Self::data("");
        event.data = None;
        event.comment = reject_untransmittable("comment", text.into(), false);
        event
    }

    /// Sets the event's `id` field. The browser sends it back as `Last-Event-ID` on reconnect.
    ///
    /// A value containing CR, LF or U+0000 is dropped rather than altered: the spec has a client
    /// ignore an `id` field containing U+0000, and an id is a resumption token, so sending a
    /// modified one would resume a reconnecting client at a position it never reached.
    pub fn id(mut self, id: impl Into<String>) -> Self {
        self.id = reject_untransmittable("id", id.into(), true);
        self
    }

    /// Sets the event type. Clients listen with `es.addEventListener("name", ...)`.
    ///
    /// A value containing CR or LF is dropped rather than altered; the event then dispatches under
    /// the default `message` type.
    pub fn event(mut self, name: impl Into<String>) -> Self {
        self.event = reject_untransmittable("event", name.into(), false);
        self
    }

    /// Suggests how long (in milliseconds) the browser should wait before reconnecting.
    pub fn retry_ms(mut self, ms: u64) -> Self {
        self.retry = Some(ms);
        self
    }

    fn encode(self) -> Bytes {
        let mut buf = String::new();
        if let Some(comment) = self.comment {
            writeln!(buf, ": {comment}").unwrap();
        }
        if let Some(id) = self.id {
            writeln!(buf, "id: {id}").unwrap();
        }
        if let Some(event) = self.event {
            writeln!(buf, "event: {event}").unwrap();
        }
        if let Some(ms) = self.retry {
            writeln!(buf, "retry: {ms}").unwrap();
        }
        // A reader rebuilds the payload by joining every `data:` value with LF and dropping one
        // trailing LF, so one line here per line break there. Empty data still writes a `data: `
        // line: the reader checks its buffer for emptiness before dropping that LF, so the buffer
        // reads as "\n" and the event dispatches carrying "".
        if let Some(data) = &self.data {
            write_data_lines(&mut buf, data);
        }
        buf.push('\n'); // blank line terminates the event
        Bytes::from(buf)
    }
}

/// Splits on the three line terminators a reader recognises, keeping a trailing empty piece.
///
/// `str::lines` treats a final terminator as optional and does not break on a lone CR, which costs
/// one trailing newline and truncates a payload at the first CR. A CR cannot be carried inside a
/// field value at all — it ends the line wherever it appears — so one becomes a line break here
/// and reaches the client as LF.
fn write_data_lines(buf: &mut String, data: &str) {
    let normalized: Cow<'_, str> = if data.contains('\r') {
        Cow::Owned(data.replace("\r\n", "\n").replace('\r', "\n"))
    } else {
        Cow::Borrowed(data)
    };
    // `split` keeps the empty piece after a trailing terminator, which is the one `lines` drops.
    for line in normalized.split('\n') {
        writeln!(buf, "data: {line}").unwrap();
    }
}

/// `Some` when the value can be transmitted, `None` when it is dropped.
///
/// CR and LF end a line wherever they appear, so a value carrying one would add fields to the
/// frame — the SSE form of header injection. There is no escape to apply, and altering the value
/// without saying so is worse than omitting it, so the field does not go out and the drop is logged.
fn reject_untransmittable(field: &str, value: String, reject_nul: bool) -> Option<String> {
    let bad = value
        .chars()
        .find(|c| *c == '\r' || *c == '\n' || (reject_nul && *c == '\0'));
    match bad {
        Some(c) => {
            tracing::warn!(
                field,
                character = ?c,
                "SSE field value cannot be carried on the wire; the field is omitted"
            );
            None
        }
        None => Some(value),
    }
}

/// An item an SSE stream may yield.
///
/// Implemented for [`SseEvent`] and for `Result<SseEvent, E>`. A stream of either is an SSE body,
/// so a handler chooses per-event fallibility by its item type and by nothing else.
///
/// # What an `Err` does on the wire
///
/// It ends the response as a failed transfer. Once a frame has been flushed the client has
/// accepted a `200`, so there is no status left to change and no envelope to write; before that,
/// the request fails with no head readable at all. SSE defines no frame for a failure either way.
///
/// Neither is distinguishable from a dropped connection, so the client reconnects after the
/// reconnection time, carrying `Last-Event-ID` if an event set one. Nothing here resumes on its
/// behalf: what it gets is whatever the route answers that new request. **An `Err` is not a way
/// to say "stop"** — it says one event could not be produced, and the caller comes back.
///
/// Stopping a client is that next request's business:
///
/// - answer `204`, the code the spec names for telling it not to return — built from
///   [`HttpResponse`] on a `#[get]` route, as [`Sse`] shows;
/// - or fail the connection another way, since any non-200 status, or a `Content-Type` that is
///   not `text/event-stream`, is not retried either.
///
/// [`SseEvent::retry_ms`] on an event sent *before* the failure sets how long it waits.
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
///
/// Answering this from `#[get]`, as above, declares nothing about the answer: an adapter that
/// collects a response body accepts the route and then never answers a stream that does not
/// end. `#[sse]` declares it and is refused on such an adapter at startup.
///
/// # Setting a header, or answering something else
///
/// The response this becomes carries `200` and three headers, and its fields are public:
///
/// ```rust,ignore
/// #[get("/events")]
/// async fn events(&self) -> HttpResponse {
///     // 204 is the code the spec names for telling a client not to reconnect. Any failed
///     // connection stops it — a non-200 status, or a `Content-Type` that is not
///     // `text/event-stream`.
///     if self.caught_up() {
///         return HttpResponse::no_content().build();
///     }
///     let mut response = HttpResponse::from(Sse::new(self.pending()));
///     response.headers.push(("X-Stream".into(), "orders".into()));
///     response
/// }
/// ```
///
/// Shaping the response means `#[get]`: `#[sse]` takes a stream, not a response. That trade is
/// real — a `#[get]` route declares nothing about its answer, so it registers on an adapter that
/// collects response bodies instead of being refused at startup.
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

impl<S> From<Sse<S>> for HttpResponse
where
    S: Stream + Send + 'static,
    S::Item: SseItem,
{
    fn from(sse: Sse<S>) -> Self {
        let encoded = sse.0.map(|item| item.into_result().map(SseEvent::encode));
        HttpResponse {
            status: 200,
            headers: vec![
                ("Content-Type".into(), "text/event-stream".into()),
                ("Cache-Control".into(), "no-cache".into()),
                // Tells nginx/caddy not to buffer the response before forwarding
                ("X-Accel-Buffering".into(), "no".into()),
            ],
            body: Some(Body::stream(encoded)),
        }
    }
}

impl<S> IntoOutput<Http> for Sse<S>
where
    S: Stream + Send + 'static,
    S::Item: SseItem,
{
    fn into_output(self) -> Answer<Http> {
        Ok(self.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reader implementing the spec's dispatch steps, which is what the encoder is written
    /// against. Asserting bytes alone would pass a frame no client raises, and asserting against
    /// `str::lines` would pass exactly the losses this module exists to avoid.
    ///
    /// Returns the data a client would deliver, or `None` where it dispatches no event.
    fn dispatched(frame: &str) -> Option<String> {
        let mut data = String::new();
        for line in frame
            .split_inclusive(['\n', '\r'])
            .map(|l| l.trim_end_matches(['\n', '\r']))
        {
            // A line beginning with a colon is a comment and contributes nothing.
            if line.starts_with(':') {
                continue;
            }
            if let Some(value) = line.strip_prefix("data:") {
                data.push_str(value.strip_prefix(' ').unwrap_or(value));
                data.push('\n');
            }
        }
        // The two steps run in this order, and the order is the whole subtlety: a single `data: `
        // line leaves the buffer as "\n", which is not empty, so the event dispatches carrying "".
        // Testing emptiness after dropping the LF would report no event for a frame a client
        // raises one for.
        if data.is_empty() {
            return None; // step 2: "If the data buffer is an empty string, ... return."
        }
        if data.ends_with('\n') {
            data.pop(); // step 3: drop one trailing LF.
        }
        Some(data)
    }

    fn encode(event: SseEvent) -> String {
        String::from_utf8(SseEvent::encode(event).to_vec()).expect("the encoder writes UTF-8")
    }

    /// The payload a handler passes in is the payload a client delivers.
    #[test]
    fn a_payload_survives_the_round_trip() {
        for payload in [
            "hello",
            "a\nb",
            "trailing space ",
            "a\n",
            "a\n\n",
            "\n",
            "",
            "line1\nline2\nline3",
            "{\"json\":true}",
        ] {
            let frame = encode(SseEvent::data(payload));
            assert_eq!(
                dispatched(&frame).as_deref(),
                Some(payload),
                "payload {payload:?} encoded as {frame:?}"
            );
        }
    }

    /// A CR cannot be carried inside a field value — it ends the line wherever it appears — so it
    /// reaches the client as a line break rather than truncating the payload at that point.
    #[test]
    fn a_carriage_return_becomes_a_line_break_rather_than_a_truncation() {
        let frame = encode(SseEvent::data("alpha\rbeta"));
        assert_eq!(dispatched(&frame).as_deref(), Some("alpha\nbeta"));

        // CRLF is one terminator, not two.
        let frame = encode(SseEvent::data("alpha\r\nbeta"));
        assert_eq!(dispatched(&frame).as_deref(), Some("alpha\nbeta"));
    }

    /// An event carrying no payload is still an event: a `data: ` line leaves the reader's buffer
    /// as a lone LF, which is not empty, so it dispatches with `""`. A payload-less typed signal
    /// depends on this.
    #[test]
    fn empty_data_dispatches_an_event_carrying_nothing() {
        let frame = encode(SseEvent::data(""));
        assert_eq!(frame, "data: \n\n");
        assert_eq!(dispatched(&frame).as_deref(), Some(""));

        let frame = encode(SseEvent::data("").event("refresh"));
        assert_eq!(frame, "event: refresh\ndata: \n\n");
        assert_eq!(dispatched(&frame).as_deref(), Some(""));
    }

    /// A JSON payload is the data line, serialized.
    #[test]
    fn json_serializes_into_the_data_field() {
        let frame = encode(SseEvent::json(&serde_json::json!({"celsius": 21.5})).unwrap());
        assert_eq!(dispatched(&frame).as_deref(), Some(r#"{"celsius":21.5}"#));
    }

    /// A comment carries no data, so it holds the connection open without raising an event.
    #[test]
    fn a_comment_holds_the_connection_without_dispatching() {
        let frame = encode(SseEvent::comment("keepalive"));
        assert_eq!(frame, ": keepalive\n\n");
        assert_eq!(dispatched(&frame), None);
    }

    /// A newline in `id` or `event` would add fields to the frame. The field is dropped, so the
    /// payload is unchanged and nothing is injected.
    #[test]
    fn a_line_terminator_in_a_field_drops_that_field() {
        for injected in [
            "x\ndata: injected",
            "x\rdata: injected",
            "x\r\nevent: other",
        ] {
            let frame = encode(SseEvent::data("real").id(injected).event(injected));
            assert_eq!(
                dispatched(&frame).as_deref(),
                Some("real"),
                "injection through {injected:?} reached the payload: {frame:?}"
            );
            assert!(!frame.contains("id:"), "id survived: {frame:?}");
            assert!(!frame.contains("event:"), "event survived: {frame:?}");
        }
    }

    /// The spec has a reader ignore an `id` field containing U+0000. Dropping it here matches that,
    /// where stripping the character would hand the client a resumption token the handler never
    /// wrote — and `"a\0b"` and `"ab"` would collapse onto one.
    #[test]
    fn a_nul_in_id_drops_the_field_and_leaves_event_alone() {
        let frame = encode(SseEvent::data("x").id("a\0b"));
        assert!(!frame.contains("id:"), "id survived: {frame:?}");

        // The spec states no NUL rule for `event`, so it travels.
        let frame = encode(SseEvent::data("x").event("a\0b"));
        assert!(
            frame.contains("event: a\0b"),
            "event was altered: {frame:?}"
        );
    }

    /// A valid field reaches the wire unchanged.
    #[test]
    fn valid_fields_are_written() {
        let frame = encode(
            SseEvent::data("payload")
                .id("42")
                .event("update")
                .retry_ms(3000),
        );
        assert!(frame.contains("id: 42\n"));
        assert!(frame.contains("event: update\n"));
        assert!(frame.contains("retry: 3000\n"));
        assert_eq!(dispatched(&frame).as_deref(), Some("payload"));
    }
}
