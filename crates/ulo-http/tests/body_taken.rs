//! A body a guard took before the handler's body extractor ran is the server's fault, not the
//! client's (QS-8): every body extractor fails with `ExtractError::BodyTaken`, rendered 500 with
//! its message withheld and logged once at `warn` naming the extractor, and `Option<..>` of one
//! fails the same way rather than reading the taken body as absent. A request whose body is empty,
//! with nothing taking it, answers as it always has.

mod o1_support;

use std::any::type_name;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use http::{Method, StatusCode};
use o1_support::{Call, Running, start};
use serde::Deserialize;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{AnyErrorHandler, BoxError, ErrorHandler, ExecutionRef, Guard, ModuleDef, injectable, routes};
use ulo_http::{BodyStream, Bytes, Form, FromCall, Http, HttpBody, HttpCx, Json, Multipart, Response, Timeout};
use ulo_transport::{CallError, ExtractError, Valid, Validate};

#[derive(Deserialize, Validate)]
struct Item {
    #[validate(length(min = 1))]
    name: String,
}

/// Takes the request body and admits the request; refuses it when there was no body to take, so a
/// test that reaches the handler knows the guard took one.
struct TakesBody;

impl Guard<Http> for TakesBody {
    async fn can_activate(&self, cx: &HttpCx) -> Result<bool, BoxError> {
        Ok(cx.take_body().is_some())
    }
}

#[injectable]
struct Taken;

#[routes]
impl Taken {
    #[ulo_http::post("/taken/json")]
    #[guards(value = TakesBody)]
    fn json(&self, item: Json<Item>) -> String {
        item.name.clone()
    }

    #[ulo_http::post("/taken/optional")]
    #[guards(value = TakesBody)]
    fn optional(&self, item: Option<Json<Item>>) -> String {
        item.map_or_else(|| "none".to_owned(), |item| item.name.clone())
    }

    #[ulo_http::post("/taken/valid")]
    #[guards(value = TakesBody)]
    fn valid(&self, item: Valid<Json<Item>>) -> String {
        item.0.name.clone()
    }

    #[ulo_http::post("/taken/form")]
    #[guards(value = TakesBody)]
    fn form(&self, item: Form<Item>) -> String {
        item.name.clone()
    }

    #[ulo_http::post("/taken/bytes")]
    #[guards(value = TakesBody)]
    fn bytes(&self, bytes: Bytes) -> String {
        bytes.len().to_string()
    }

    #[ulo_http::post("/taken/optional-bytes")]
    #[guards(value = TakesBody)]
    fn optional_bytes(&self, bytes: Option<Bytes>) -> String {
        bytes.map_or_else(|| "none".to_owned(), |bytes| bytes.len().to_string())
    }

    #[ulo_http::post("/taken/stream")]
    #[guards(value = TakesBody)]
    fn stream(&self, _body: BodyStream) -> &'static str {
        "read"
    }

    #[ulo_http::post("/taken/multipart")]
    #[guards(value = TakesBody)]
    fn multipart(&self, _parts: Multipart) -> &'static str {
        "read"
    }

    #[ulo_http::post("/plain/json")]
    fn plain_json(&self, item: Json<Item>) -> String {
        item.name.clone()
    }

    #[ulo_http::post("/plain/optional")]
    fn plain_optional(&self, item: Option<Json<Item>>) -> String {
        item.map_or_else(|| "none".to_owned(), |item| item.name.clone())
    }

    #[ulo_http::post("/timed")]
    #[meta(Timeout::after(Duration::from_millis(20)))]
    #[error_handlers(value = ReadsBodyTwice)]
    async fn timed(&self, exec: ExecutionRef) -> &'static str {
        exec.cancelled().await;
        "late"
    }
}

/// Reads the body twice from the context a route timeout's error handlers get, which holds none:
/// absent rather than taken, so each read is `None` and nothing is logged.
struct ReadsBodyTwice;

impl ErrorHandler<Http> for ReadsBodyTwice {
    async fn handle(&self, _err: BoxError, cx: &HttpCx) -> Result<Response, BoxError> {
        let mut seen = Vec::new();
        for _ in 0..2 {
            seen.push(match <Option<Bytes> as FromCall<Http>>::from_call(cx).await {
                Ok(None) => "none".to_owned(),
                Ok(Some(bytes)) => format!("{} bytes", bytes.len()),
                Err(error) => error.to_string(),
            });
        }
        Ok(Response::new(HttpBody::from(seen.join("|"))))
    }
}

/// Answers every error with 200 and the `ExtractError` it carried, so the variant shows.
struct Report;

impl ErrorHandler<Http> for Report {
    async fn handle(&self, err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        let call = CallError::from_boxed(err);
        let text = match call.source_as::<ExtractError>() {
            Some(ExtractError::BodyTaken { param, extractor }) => format!("BodyTaken|{param}|{extractor}"),
            Some(ExtractError::Missing { param }) => format!("Missing|{param}"),
            Some(other) => format!("other|{other}"),
            None => format!("not an extract error|{}", call.kind()),
        };
        Ok(Response::new(HttpBody::from(text)))
    }
}

fn plain(m: &mut ModuleDef<'_>) {
    m.controller::<Taken>();
}

fn reporting(m: &mut ModuleDef<'_>) {
    plain(m);
    let report: Arc<AnyErrorHandler<Http>> = Arc::new(Report);
    m.enhancer::<AnyErrorHandler<Http>>().value(report);
}

const UPLOAD: &str = "--XB\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\nann\r\n--XB--\r\n";

/// Each guarded route, the request it gets, and the extractor whose read finds the body gone.
fn taken_routes() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    let json = type_name::<Json<Item>>();
    vec![
        ("/taken/json", "application/json", r#"{"name":"ann"}"#, json),
        ("/taken/optional", "application/json", r#"{"name":"ann"}"#, json),
        ("/taken/valid", "application/json", r#"{"name":"ann"}"#, json),
        ("/taken/form", "application/x-www-form-urlencoded", "name=ann", type_name::<Form<Item>>()),
        ("/taken/bytes", "application/octet-stream", "ann", type_name::<Bytes>()),
        ("/taken/optional-bytes", "application/octet-stream", "ann", type_name::<Bytes>()),
        ("/taken/stream", "application/octet-stream", "ann", type_name::<BodyStream>()),
        ("/taken/multipart", "multipart/form-data; boundary=XB", UPLOAD, type_name::<Multipart>()),
    ]
}

async fn post(app: &Running, path: &str, content_type: &str, body: &'static str) -> o1_support::Reply {
    app.send(Call::new(Method::POST, path).header("content-type", content_type).body(body)).await
}

#[tokio::test]
async fn a_body_a_guard_took_fails_every_body_extractor_with_body_taken_as_a_500_and_one_warn_naming_it() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, seen) = (start(plain).await, start(reporting).await);

    for (path, content_type, body, extractor) in taken_routes() {
        capture.take();
        let reply = post(&app, path, content_type, body).await;
        assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{path}: {}", reply.text());
        assert_eq!(reply.header("content-type"), Some("application/problem+json"), "{path}");
        let problem = reply.json();
        assert_eq!(problem["status"], 500, "{path}");
        assert!(!problem["detail"].as_str().unwrap_or("").contains("Item"), "{path}: the client is told no type: {problem}");

        let named = format!("extractor={extractor}");
        let warnings: Vec<String> = capture.take().into_iter().filter(|line| line.contains(&named)).collect();
        assert_eq!(warnings.len(), 1, "{path}: one `warn` naming `{extractor}`: {warnings:?}");
        assert!(warnings[0].starts_with("WARN the request body was taken before the extractor ran"), "{path}: {warnings:?}");
        assert!(warnings[0].contains(&format!("route={path}")), "{path}: the warn names the route: {warnings:?}");
        assert!(!warnings[0].contains("ann"), "{path}: the warn carries no body content: {warnings:?}");

        let report = post(&seen, path, content_type, body).await;
        assert_eq!(report.text(), format!("BodyTaken|body|{extractor}"), "{path}: what the error handlers see");
    }
    app.stop().await;
    seen.stop().await;
}

#[tokio::test]
async fn an_empty_body_nothing_took_is_still_missing_and_an_optional_body_still_reads_none() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let (app, seen) = (start(plain).await, start(reporting).await);

    let empty = app.send(Call::new(Method::POST, "/plain/json").header("content-type", "application/json")).await;
    assert_eq!(empty.status, StatusCode::BAD_REQUEST, "{}", empty.text());
    assert_eq!(empty.json()["detail"], "`body` is missing");
    let report = seen.send(Call::new(Method::POST, "/plain/json").header("content-type", "application/json")).await;
    assert_eq!(report.text(), "Missing|body");

    let none = app.send(Call::new(Method::POST, "/plain/optional").header("content-type", "application/json")).await;
    assert_eq!((none.status, none.text()), (StatusCode::OK, "none".to_owned()));
    let some = post(&app, "/plain/optional", "application/json", r#"{"name":"ann"}"#).await;
    assert_eq!((some.status, some.text()), (StatusCode::OK, "ann".to_owned()));

    // The request carries a body; the context the timeout's error handler reads holds none.
    let timed = post(&app, "/timed", "application/octet-stream", "ann").await;
    assert_eq!((timed.status, timed.text()), (StatusCode::OK, "none|none".to_owned()), "an absent body is not a taken one");

    let warnings = capture.take();
    assert!(warnings.iter().all(|line| !line.contains("extractor=")), "nothing took a body: {warnings:?}");
    app.stop().await;
    seen.stop().await;
}

/// Every `warn` and `error` event on the thread it is the default subscriber of, as its level, its
/// message, then `name=value` for each other field.
#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<String>>>,
    next_span: Arc<AtomicU64>,
}

impl Capture {
    /// The lines since the last take.
    fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.lines.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_span() || *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(self.next_span.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        let text = format!("{} {}{}", event.metadata().level(), line.message, line.fields);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push(text);
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_owned();
        } else {
            self.fields.push_str(&format!(" {}={value}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}
