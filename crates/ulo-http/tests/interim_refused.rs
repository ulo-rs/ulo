//! A final 1xx reaches the backend only as an `UpgradeHandler`'s 101 (transports DESIGN decision
//! 280): a handler, a pre-dispatch entry or an error handler answering one, and an upgrade
//! handler answering any other 1xx, are answered 500 as the internal-error problem document,
//! logged once at `error`, the handler's by its route and name. The tracing capture lives in this
//! file alone, so no callsite here is first reached on a thread without a subscriber.

mod o1_support;

use std::borrow::Cow;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use http::StatusCode;
use o1_support::{Call, start};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{AnyErrorHandler, BoxError, BoxFuture, ErrorHandler, ModuleDef, controller, handlers, injectable};
use ulo_http::middleware::{Middleware, Next};
use ulo_http::{Http, HttpBody, HttpCx, Path, PreDispatch, Request, Response, UpgradeHandler, Upgrades};

#[controller]
struct Interim;

#[handlers]
impl Interim {
    #[ulo_http::get("/status/{code}")]
    fn status(&self, code: Path<u16>) -> Response {
        with_status(code.0)
    }

    #[ulo_http::get("/plain")]
    fn plain(&self) -> &'static str {
        "plain"
    }
}

fn with_status(code: u16) -> Response {
    let mut response = Response::new(HttpBody::from("interim"));
    *response.status_mut() = StatusCode::from_u16(code).expect("a status code");
    response
}

/// Answers an upgrade request on `/socket` with the status its `x-answer` header names.
struct Socket;

impl UpgradeHandler for Socket {
    fn paths(&self, _app: &ulo::AppHandle) -> Vec<Cow<'static, str>> {
        vec!["/socket".into()]
    }

    fn upgrade(&self, req: Request) -> BoxFuture<'static, Response> {
        let code = req.headers().get("x-answer").and_then(|value| value.to_str().ok()?.parse().ok()).unwrap_or(101);
        Box::pin(async move { with_status(code) })
    }
}

/// Answers a request carrying `x-entry` itself, with the status that header names, without `next`.
#[injectable]
struct Entry;

impl Middleware for Entry {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        match req.headers().get("x-entry").and_then(|value| value.to_str().ok()?.parse().ok()) {
            Some(code) => Ok(with_status(code)),
            None => next.run(req).await,
        }
    }
}

/// Over every path, the upgrade handler's on `/socket` among them: replaces the answer `next`
/// gives with one of the status the request's `x-replace` names.
#[injectable]
struct Replace;

impl Middleware for Replace {
    async fn handle(&self, req: Request, next: Next<'_>) -> Result<Response, BoxError> {
        let replace = req.headers().get("x-replace").and_then(|value| value.to_str().ok()?.parse().ok());
        let response = next.run(req).await?;
        Ok(replace.map_or(response, with_status))
    }
}

/// Answers every miss with 101.
struct SwitchOnMiss;

impl ErrorHandler<Http> for SwitchOnMiss {
    async fn handle(&self, _err: BoxError, _cx: &HttpCx) -> Result<Response, BoxError> {
        Ok(with_status(101))
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Interim>();
    m.provide::<Entry>();
    m.provide::<Replace>();
    m.meta::<PreDispatch>().apply::<Entry>().apply_for::<Replace>(["/*"]);
    m.meta::<Upgrades>().register(Socket);
    let switch: Arc<AnyErrorHandler<Http>> = Arc::new(SwitchOnMiss);
    m.enhancer::<AnyErrorHandler<Http>>().value(switch);
}

fn assert_refused(reply: &o1_support::Reply, what: &str) {
    assert_eq!(reply.status, StatusCode::INTERNAL_SERVER_ERROR, "{what}: {}", reply.text());
    assert_eq!(reply.header("content-type"), Some("application/problem+json"), "{what}");
    assert_eq!(reply.json()["detail"], "internal error", "{what}");
}

#[tokio::test]
async fn a_final_1xx_from_anything_but_an_upgrade_handler_s_101_is_answered_500_logged_at_error() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let app = start(register).await;

    for code in [100u16, 101, 102, 103, 150, 199] {
        capture.take();
        let reply = app.send(Call::get(&format!("/status/{code}"))).await;
        assert_refused(&reply, &format!("a handler's {code}"));
        assert_eq!(reply.matched(), Some(("/status/{code}".to_owned(), "status")), "the routing the handler's call decided stays");
        let errors = capture.take();
        assert_eq!(errors.len(), 1, "a handler's {code}: {errors:?}");
        assert!(errors[0].starts_with("ERROR a handler answered with a 1xx status"), "{errors:?}");
        assert!(errors[0].contains(&format!("status={code}")), "{errors:?}");
        assert!(errors[0].contains("route=/status/{code}") && errors[0].contains("handler=Interim::status"), "the error names the route and the handler: {errors:?}");
    }

    capture.take();
    let entry = app.send(Call::get("/plain").header("x-entry", "101")).await;
    assert_refused(&entry, "a pre-dispatch entry's 101");
    let errors = capture.take();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].starts_with("ERROR a pre-dispatch entry or an error handler answered with a 1xx status"), "{errors:?}");

    let miss = app.send(Call::get("/nowhere")).await;
    assert_refused(&miss, "an error handler's 101 for a miss");

    let switched = app.send(Call::get("/socket").header("upgrade", "websocket")).await;
    assert_eq!(switched.status, StatusCode::SWITCHING_PROTOCOLS, "an upgrade handler's 101 is the switch: {}", switched.text());
    let early = app.send(Call::get("/socket").header("upgrade", "websocket").header("x-answer", "103")).await;
    assert_refused(&early, "an upgrade handler's 103");
    let replaced = app.send(Call::get("/socket").header("upgrade", "websocket").header("x-answer", "400").header("x-replace", "101")).await;
    assert_refused(&replaced, "a scoped entry's 101 in place of the upgrade handler's 400");
    let replaced = app.send(Call::get("/socket").header("upgrade", "websocket").header("x-replace", "103")).await;
    assert_refused(&replaced, "a scoped entry's 103 in place of the upgrade handler's 101");
    let ahead = app.send(Call::get("/socket").header("upgrade", "websocket").header("x-entry", "101")).await;
    assert_refused(&ahead, "an unscoped entry's 101 on an upgrade path, ahead of the upgrade handler");

    capture.take();
    let fine = app.send(Call::get("/status/204")).await;
    assert_eq!(fine.status, StatusCode::NO_CONTENT);
    assert!(capture.take().is_empty(), "a final status logs nothing");
    app.stop().await;
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
