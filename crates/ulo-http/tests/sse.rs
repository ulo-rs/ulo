//! The Server-Sent Events wire: headers, the byte framing of `data`, `id`, `event`, `retry` and
//! comment fields, line splitting on every terminator, empty data, `end_event`, `keep_alive`,
//! the `error` event a failing item ends in, the field validation of `EventId` and `EventName`,
//! and the `LastEventId` extractor.

mod o1_support;

use std::collections::HashMap;
use std::future::poll_fn;
use std::pin::Pin;
use std::time::Duration;

use futures_util::stream::{self, BoxStream};
use futures_util::StreamExt;
use http::StatusCode;
use http_body::Body as _;
use o1_support::{Call, PATIENCE, start};
use ulo::{ModuleDef, injectable, routes};
use ulo_http::{Event, EventId, EventName, LastEventId, Sse};
use ulo_transport::{CallError, ErrorKind};

type Events = Sse<stream::Iter<std::vec::IntoIter<Event>>>;

fn events(events: Vec<Event>) -> Events {
    Sse::new(stream::iter(events))
}

#[injectable]
struct Feeds;

#[routes]
impl Feeds {
    #[ulo_http::get("/framing")]
    fn framing(&self) -> Events {
        let id = EventId::new("7").expect("a valid id");
        let name = EventName::new("tick").expect("a valid name");
        events(vec![Event::default().comment("note").event(name).id(id).retry(Duration::from_millis(1500)).data("hello")])
    }

    #[ulo_http::get("/multiline")]
    fn multiline(&self) -> Events {
        events(vec![
            Event::default().data("a\r\nb\rc\nd"),
            Event::default().data("tail\n"),
            Event::default().comment("x\ny"),
        ])
    }

    #[ulo_http::get("/empty")]
    fn empty(&self) -> Events {
        events(vec![Event::default().data(""), Event::default().comment("ping"), Event::default()])
    }

    #[ulo_http::get("/json")]
    fn json(&self) -> Events {
        events(vec![Event::default().json(&serde_json::json!({"a": 1, "b": [true]})).expect("serializes")])
    }

    #[ulo_http::get("/end")]
    fn end(&self) -> Events {
        events(vec![Event::default().data("x")]).end_event("done")
    }

    #[ulo_http::get("/bad-end")]
    fn bad_end(&self) -> Events {
        events(vec![Event::default().data("x")]).end_event("two\nlines")
    }

    #[ulo_http::get("/idle")]
    fn idle(&self) -> Sse<BoxStream<'static, Event>> {
        let first = stream::once(async { Event::default().data("first") });
        Sse::new(first.chain(stream::pending()).boxed()).keep_alive(Duration::from_millis(50))
    }

    #[ulo_http::get("/fails")]
    fn fails(&self) -> Sse<stream::Iter<std::vec::IntoIter<Result<Event, CallError>>>> {
        Sse::new(stream::iter(vec![
            Ok(Event::default().data("one")),
            Err(CallError::new(ErrorKind::Unavailable, "gone")),
            Ok(Event::default().data("never written")),
        ]))
    }

    #[ulo_http::get("/resume")]
    fn resume(&self, last: LastEventId) -> String {
        match last.0 {
            Some(id) => format!("id:{id}"),
            None => "none".to_owned(),
        }
    }
}

fn feeds(m: &mut ModuleDef<'_>) {
    m.controller::<Feeds>();
}

async fn body_of(path: &str) -> (http::HeaderMap, String) {
    let app = start(feeds).await;
    let reply = app.send(Call::get(path)).await;
    assert_eq!(reply.status, StatusCode::OK, "{path}");
    app.stop().await;
    let text = reply.text();
    (reply.headers, text)
}

#[tokio::test]
async fn the_stream_is_text_event_stream_uncached_and_one_event_per_blank_line() {
    let (headers, body) = body_of("/framing").await;

    assert_eq!(headers.get("content-type").and_then(|value| value.to_str().ok()), Some("text/event-stream; charset=utf-8"));
    assert_eq!(headers.get("cache-control").and_then(|value| value.to_str().ok()), Some("no-cache"));
    assert_eq!(body, ": note\nevent: tick\nid: 7\nretry: 1500\ndata: hello\n\n");
}

#[tokio::test]
async fn data_and_comments_split_on_cr_lf_and_crlf_and_keep_the_piece_after_a_trailing_terminator() {
    let (_, body) = body_of("/multiline").await;

    assert_eq!(body, "data: a\ndata: b\ndata: c\ndata: d\n\ndata: tail\ndata: \n\n: x\n: y\n\n");
}

#[tokio::test]
async fn empty_data_writes_an_empty_data_line_and_a_comment_alone_writes_no_data_field() {
    let (_, body) = body_of("/empty").await;

    // An event with nothing at all is just the terminating blank line.
    assert_eq!(body, "data: \n\n: ping\n\n\n");
}

#[tokio::test]
async fn a_json_event_carries_the_serialized_value_as_its_data() {
    let (_, body) = body_of("/json").await;

    assert_eq!(body, "data: {\"a\":1,\"b\":[true]}\n\n");
}

#[test]
fn json_refuses_a_map_key_that_is_not_a_string() {
    let value: HashMap<(u8, u8), u8> = HashMap::from([((1, 2), 3)]);

    assert!(Event::default().json(&value).is_err());
}

#[tokio::test]
async fn end_event_closes_the_stream_with_a_named_event_and_empty_data() {
    let (_, body) = body_of("/end").await;

    assert_eq!(body, "data: x\n\nevent: done\ndata: \n\n");
}

#[tokio::test]
async fn an_end_event_name_with_a_line_break_fails_the_reply_before_anything_is_written() {
    let app = start(feeds).await;

    let reply = app.send(Call::get("/bad-end")).await;
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_ne!(reply.header("content-type"), Some("text/event-stream; charset=utf-8"));
    app.stop().await;
}

#[tokio::test]
async fn a_failing_item_ends_the_stream_in_an_error_event_carrying_its_kind() {
    let (_, body) = body_of("/fails").await;

    let (first, rest) = body.split_once("\n\n").expect("a first event");
    assert_eq!(first, "data: one");
    let rest = rest.strip_suffix("\n\n").expect("the error event ends in a blank line");
    let data = rest.strip_prefix("event: error\ndata: ").unwrap_or_else(|| panic!("not an error event: {rest:?}"));
    let envelope: serde_json::Value = serde_json::from_str(data).expect("the error event's data is JSON");
    assert_eq!(envelope["kind"], ErrorKind::Unavailable.as_str());
    assert!(!body.contains("never written"), "{body:?}");
}

#[tokio::test]
async fn keep_alive_writes_a_comment_frame_while_the_stream_is_idle_and_again_after_it() {
    let app = start(feeds).await;
    let response = app.call(Call::get("/idle")).await;
    let (_, mut body) = response.into_parts();

    let mut frames = Vec::new();
    let started = std::time::Instant::now();
    let read = async {
        while frames.iter().filter(|frame: &&String| frame.as_str() == ": keepalive\n\n").count() < 2 {
            let frame = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await.expect("the stream is not over").expect("no error");
            if let Ok(data) = frame.into_data() {
                frames.push(String::from_utf8_lossy(&data).into_owned());
            }
        }
    };
    tokio::time::timeout(PATIENCE, read).await.expect("two keepalives arrived in time");
    let elapsed = started.elapsed();
    drop(body);

    // A lower bound only: each comment waits at least one 50 ms period, never less.
    assert!(elapsed >= Duration::from_millis(100), "two keepalives took {elapsed:?}, under two periods");
    assert_eq!(frames[0], "data: first\n\n", "the event comes before any keepalive");
    assert!(frames[1..].iter().all(|frame| frame == ": keepalive\n\n"), "{frames:?}");
    app.stop().await;
}

#[test]
fn an_id_or_event_name_holding_a_line_break_or_an_id_holding_nul_is_refused_at_construction() {
    for bad in ["a\nb", "a\rb", "a\0b"] {
        let error = EventId::new(bad).expect_err("an id with a forbidden character");
        assert!(error.to_string().contains("`id`"), "{bad:?}: {error}");
    }
    for bad in ["a\nb", "a\rb"] {
        let error = EventName::new(bad).expect_err("a name with a line break");
        assert!(error.to_string().contains("`event`"), "{bad:?}: {error}");
    }
    assert_eq!(EventId::new("42").expect("a plain id").as_str(), "42");
    assert_eq!(EventName::new("tick").expect("a plain name").as_str(), "tick");
    EventName::new("a\0b").expect("NUL is allowed in an event name");
}

#[tokio::test]
async fn last_event_id_hands_over_what_the_client_sent_and_none_on_a_first_connection() {
    let app = start(feeds).await;

    let first = app.send(Call::get("/resume")).await;
    assert_eq!((first.status, first.text()), (StatusCode::OK, "none".to_owned()));
    let again = app.send(Call::get("/resume").header("last-event-id", "42")).await;
    assert_eq!((again.status, again.text()), (StatusCode::OK, "id:42".to_owned()));
    let empty = app.send(Call::get("/resume").header("last-event-id", "")).await;
    assert_eq!(empty.text(), "id:", "an empty id is the client's reset, handed over as sent");
    app.stop().await;
}

#[tokio::test]
async fn last_event_id_that_is_not_utf8_is_a_bad_request() {
    let app = start(feeds).await;

    let reply = app.send(Call::get("/resume").header_bytes("last-event-id", &[0xff, 0xfe])).await;
    assert_eq!(reply.status, StatusCode::BAD_REQUEST);
    app.stop().await;
}
