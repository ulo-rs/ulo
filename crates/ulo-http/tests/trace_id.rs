//! The HTTP call span records `trace_id` (transports DESIGN §2.9): the caller's trace ID when the
//! request carries a `traceparent`, and one generated for the call otherwise, which the handler's
//! execution does not carry, so nothing it calls out to is handed it. The sources and the format
//! are pinned against `span::call` itself in `ulo-transport/tests/trace_id.rs`.

mod common;

use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Metadata};
use ulo::{App, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_http::{ConnInfo, HttpBody, Request};

use common::{Keeper, written};

/// Whether each handler's execution carried a trace context, in call order.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<bool>>>);

#[controller]
struct Traced {
    seen: Dep<Seen>,
}

#[handlers]
impl Traced {
    #[ulo_http::get("/traced")]
    fn traced(&self, exec: ExecutionRef) -> &'static str {
        self.seen.0.lock().unwrap_or_else(PoisonError::into_inner).push(exec.trace().is_some());
        "ok"
    }
}

struct Root(Seen);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Traced>();
    }
}

/// Every `trace_id` a span recorded.
#[derive(Clone, Default)]
struct Ids(Arc<Mutex<Vec<String>>>);

struct TraceIdField<'a>(&'a Ids);

impl Visit for TraceIdField<'_> {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "trace_id" {
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

fn get(traceparent: Option<&str>) -> Request {
    let mut builder = http::Request::get("/traced");
    if let Some(value) = traceparent {
        builder = builder.header("traceparent", value);
    }
    let (head, ()) = builder.body(()).expect("the request builds").into_parts();
    Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None }
}

#[tokio::test(flavor = "current_thread")]
async fn the_call_span_records_the_callers_trace_id_or_one_of_its_own() {
    let ids = Ids::default();
    let _recorded = tracing::subscriber::set_default(ids.clone());
    let seen = Seen::default();
    let keeper = Keeper::default();
    let _app = App::builder(Root(seen.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()))
        .listen()
        .await
        .expect("the app listens");
    let svc = keeper.service();

    let (status, _) = written(svc.call(get(Some("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01"))).await).await;
    assert_eq!(status, 200);
    let (status, _) = written(svc.call(get(None)).await).await;
    assert_eq!(status, 200);

    let recorded = ids.0.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let [caller, own] = recorded.as_slice() else { panic!("expected one `trace_id` per call: {recorded:?}") };
    assert_eq!(caller, "4bf92f3577b34da6a3ce929d0e0e4736", "the caller's trace ID");
    assert!(
        own.len() == 32 && own.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) && own.bytes().any(|b| b != b'0'),
        "a generated trace ID in W3C form: {own:?}"
    );
    assert_eq!(*seen.0.lock().unwrap_or_else(PoisonError::into_inner), vec![true, false], "a generated trace ID reached the execution");
}
