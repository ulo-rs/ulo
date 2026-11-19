//! The call span's `trace_id` (transports DESIGN §2.9): the execution's context's trace ID when it
//! has one, a bound `TraceSource`'s answer or the caller's `traceparent` in pass-through, and a
//! generated one otherwise, in W3C form, 32 lowercase hex digits and never all zeros. A generated
//! ID stays on the span: the execution carries no context afterwards, so a client called
//! `.within` it writes no `traceparent`.

use std::sync::{Arc, Mutex, PoisonError};

use tracing::Span;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata};
use ulo::{App, AppHandle, Connected, ExecOptions, Execution, Module, ModuleDef, ModuleIdentity, TraceContext, TraceMode, TraceSource, TraceState};
use ulo_transport::span;

const INBOUND: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const INBOUND_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const SOURCE_ID: [u8; 16] = [0x5a; 16];

/// Every span's `trace_id`, recorded at creation or after it.
#[derive(Clone, Default)]
struct Ids(Arc<Mutex<Vec<String>>>);

impl Ids {
    fn taken(&self) -> Vec<String> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

struct TraceIdField<'a>(&'a Ids);

impl Visit for TraceIdField<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == span::TRACE_ID {
            self.0.0.lock().unwrap_or_else(PoisonError::into_inner).push(value.to_owned());
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl tracing::Subscriber for Ids {
    fn enabled(&self, _: &Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, attrs: &Attributes<'_>) -> Id {
        attrs.record(&mut TraceIdField(self));
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, values: &Record<'_>) {
        values.record(&mut TraceIdField(self));
    }
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, _: &Event<'_>) {}
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

/// Answers a context with [`SOURCE_ID`] whatever it is handed.
struct Fixed;

impl TraceSource for Fixed {
    fn start(&self, _span: &Span, _inbound: Option<&TraceContext>) -> TraceContext {
        TraceContext::new(SOURCE_ID, [0x07; 8], 0x01, TraceState::new()).expect("a valid context")
    }
}

struct Root {
    source: bool,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        if self.source {
            m.value(Fixed).also_as::<dyn TraceSource>(|source| source);
        }
    }
}

/// A connected app; the test holds it while it calls, a handle being a view the app's drop closes.
fn app(source: bool, mode: TraceMode) -> App<Connected> {
    futures_executor::block_on(App::builder(Root { source }).trace(mode).wire().expect("the app wires").connect()).expect("the app connects")
}

/// The `trace_id` the call span records for one execution opened with `inbound`, and the
/// execution's context once `span::call` has returned.
fn call(app: &AppHandle, inbound: Option<&str>) -> (String, Option<TraceContext>) {
    let mut opts = ExecOptions::new();
    if let Some(traceparent) = inbound {
        opts = opts.trace(TraceContext::extract([traceparent], []).expect("a valid traceparent"));
    }
    let exec = Execution::open(&app.root(), opts).expect("the execution opens");
    let ids = Ids::default();
    tracing::subscriber::with_default(ids.clone(), || {
        let _span = span::call(app, &exec, span::Kind::Server, "test", "GET /", None);
    });
    let recorded = ids.taken();
    assert_eq!(recorded.len(), 1, "the span recorded `trace_id` once: {recorded:?}");
    (recorded.into_iter().next().unwrap_or_default(), exec.trace().cloned())
}

fn is_w3c_trace_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) && id.bytes().any(|b| b != b'0')
}

#[test]
fn with_no_context_a_trace_id_is_generated_per_call_and_the_execution_carries_none() {
    let app = app(false, TraceMode::On);
    let (first, context) = call(&app.handle(), None);
    let (second, _) = call(&app.handle(), None);
    assert!(is_w3c_trace_id(&first), "not a W3C trace ID: {first:?}");
    assert!(is_w3c_trace_id(&second), "not a W3C trace ID: {second:?}");
    assert_ne!(first, second, "two calls took one generated trace ID");
    assert_eq!(context, None, "a generated trace ID became the execution's context, which a client would propagate");
}

#[test]
fn in_pass_through_the_trace_id_is_the_callers() {
    let app = app(false, TraceMode::On);
    let (id, context) = call(&app.handle(), Some(INBOUND));
    assert_eq!(id, INBOUND_ID);
    assert_eq!(context.map(|context| context.traceparent()), Some(INBOUND.to_owned()), "the inbound context is forwarded as received");
}

#[test]
fn with_a_source_bound_the_trace_id_is_the_sources_answer() {
    let app = app(true, TraceMode::On);
    for inbound in [None, Some(INBOUND)] {
        let (id, context) = call(&app.handle(), inbound);
        assert_eq!(id, "5a".repeat(16), "inbound {inbound:?}");
        assert_eq!(context.map(|context| context.trace_id()), Some(SOURCE_ID), "inbound {inbound:?}");
    }
}

#[test]
fn with_tracing_off_an_incoming_context_is_ignored_and_the_id_is_generated() {
    let app = app(false, TraceMode::Off);
    let (id, context) = call(&app.handle(), Some(INBOUND));
    assert!(is_w3c_trace_id(&id), "not a W3C trace ID: {id:?}");
    assert_ne!(id, INBOUND_ID, "a context read under `TraceMode::Off`");
    assert_eq!(context, None);
}

#[test]
fn the_check_refuses_what_is_not_a_trace_id() {
    for bad in ["0".repeat(32), "4BF92F3577B34DA6A3CE929D0E0E4736".to_owned(), "4bf92f3577b34da6a3ce929d0e0e473".to_owned(), String::new()] {
        assert!(!is_w3c_trace_id(&bad), "{bad:?} passed");
    }
}
