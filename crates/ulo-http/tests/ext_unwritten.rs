//! A handler parameter `Ext<T>` whose extension nothing wrote earlier in the execution answers 500
//! and is logged: at `warn` the first time for each handler and parameter type in the process, at
//! `debug` after, naming the parameter's type and the extension's. A `Dep<T>` whose execution-scoped
//! construction reads the unwritten extension is logged the same way. An `Option<Ext<T>>` reads the
//! absence as `None` and logs nothing.
//!
//! One test in this binary: the set of handlers already warned about is the process's, and a
//! `tracing` callsite first reached on a thread with no subscriber can keep the interest it
//! computed without one.

mod o1_support;

use std::any::type_name;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use http::StatusCode;
use o1_support::{Call, start};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{Dep, Ext, ModuleDef, controller, handlers};

struct Session;
struct CurrentUser;

#[controller]
struct Pages;

#[handlers]
impl Pages {
    #[ulo_http::get("/inbox")]
    fn inbox(&self, _session: Ext<Session>) -> &'static str {
        "inbox"
    }

    #[ulo_http::get("/outbox")]
    fn outbox(&self, _session: Ext<Session>) -> &'static str {
        "outbox"
    }

    #[ulo_http::get("/me")]
    fn me(&self, _user: Dep<CurrentUser>) -> &'static str {
        "me"
    }

    #[ulo_http::get("/maybe")]
    fn maybe(&self, session: Option<Ext<Session>>) -> &'static str {
        if session.is_some() { "signed in" } else { "anonymous" }
    }
}

fn register(m: &mut ModuleDef<'_>) {
    m.controller::<Pages>();
    m.execution(|_session: Ext<Session>| async move { CurrentUser });
}

#[tokio::test(flavor = "current_thread")]
async fn an_unwritten_extension_answers_500_and_warns_once_per_handler_then_debugs() {
    let capture = Capture::default();
    let _default = tracing::subscriber::set_default(capture.clone());
    let app = start(register).await;
    let param = format!("param={}", type_name::<Ext<Session>>());
    let unwritten = |lines: &[String]| -> Vec<String> { lines.iter().filter(|line| line.contains(&param)).cloned().collect() };

    let first = app.send(Call::get("/inbox")).await;
    assert_eq!(first.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", first.text());
    assert!(!first.text().contains("Session"), "the client is told no type: {}", first.text());
    let lines = unwritten(&capture.take());
    assert_eq!(lines.len(), 1, "one event for the miss: {lines:?}");
    assert!(lines[0].starts_with("WARN nothing wrote the extension this handler parameter reads earlier in the execution"), "{lines:?}");
    assert!(lines[0].contains("handler=Pages::inbox"), "the warn names the handler: {lines:?}");
    assert!(lines[0].contains("route=/inbox"), "the warn names the route: {lines:?}");
    assert!(lines[0].contains(&format!("extension={}", type_name::<Session>())), "the warn names the extension: {lines:?}");

    for _ in 0..2 {
        let again = app.send(Call::get("/inbox")).await;
        assert_eq!(again.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", again.text());
        let lines = unwritten(&capture.take());
        assert_eq!(lines.len(), 1, "one event per miss: {lines:?}");
        assert!(lines[0].starts_with("DEBUG nothing wrote the extension"), "a later miss on the same handler is `debug`: {lines:?}");
    }

    let other = app.send(Call::get("/outbox")).await;
    assert_eq!(other.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", other.text());
    let lines = unwritten(&capture.take());
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].starts_with("WARN ") && lines[0].contains("handler=Pages::outbox"), "another handler warns once of its own: {lines:?}");

    let dependency = app.send(Call::get("/me")).await;
    assert_eq!(dependency.status, StatusCode::INTERNAL_SERVER_ERROR, "{}", dependency.text());
    let param = format!("param={}", type_name::<Dep<CurrentUser>>());
    let lines: Vec<String> = capture.take().into_iter().filter(|line| line.contains(&param)).collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(
        lines[0].starts_with("WARN ") && lines[0].contains(&format!("extension={}", type_name::<Session>())),
        "a dependency built from the unwritten extension names the extension: {lines:?}"
    );

    let optional = app.send(Call::get("/maybe")).await;
    assert_eq!((optional.status, optional.text()), (StatusCode::OK, "anonymous".to_owned()));
    assert_eq!(unwritten(&capture.take()), Vec::<String>::new(), "an optional extension that is absent logs nothing");
    app.stop().await;
}

/// Every event at `debug` or above on the thread it is the default subscriber of, as its level,
/// its message, then `name=value` for each other field.
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
        metadata.is_span() || *metadata.level() <= Level::DEBUG
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
