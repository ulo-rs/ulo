//! A request shed over `max_inflight` is counted into the server's `ShedLog` (transports DESIGN
//! §2.8): the first refusal writes a `warn` naming the transport at once, and the refusals after
//! it within the interval are written when the server drops with the app, none lost. The interval
//! itself is pinned on a scripted clock in `ulo-transport`'s unit tests.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{App, Dep, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_http::{ConnInfo, HttpBody, Request, StatusCode};
use ulo_transport::Count;

use common::{Keeper, written};

const PATIENCE: Duration = Duration::from_secs(10);

#[derive(Clone, Default)]
struct Gate {
    held: Arc<AtomicUsize>,
    open: Arc<Notify>,
}

#[controller]
struct Held {
    gate: Dep<Gate>,
}

#[handlers]
impl Held {
    #[ulo_http::get("/held")]
    async fn held(&self) -> &'static str {
        let opened = self.gate.open.notified();
        self.gate.held.fetch_add(1, Ordering::SeqCst);
        opened.await;
        "released"
    }
}

struct Root(Gate);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Held>();
    }
}

/// Every `ShedLog` line written at `warn`, as its `ulo.transport` and `shed` fields.
#[derive(Clone, Default)]
struct ShedLines(Arc<Mutex<Vec<(String, u64)>>>);

impl ShedLines {
    fn written(&self) -> Vec<(String, u64)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Default)]
struct ShedLine {
    transport: String,
    shed: Option<u64>,
}

impl Visit for ShedLine {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "ulo.transport" {
            self.transport = value.to_owned();
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "shed" {
            self.shed = Some(value);
        }
    }

    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

impl tracing::Subscriber for ShedLines {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event() && *metadata.level() == Level::WARN
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut line = ShedLine::default();
        event.record(&mut line);
        if let Some(shed) = line.shed {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((line.transport, shed));
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn get(path: &str) -> Request {
    let (head, ()) = http::Request::builder().uri(path).body(()).expect("the request builds").into_parts();
    Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None }
}

#[tokio::test(flavor = "current_thread")]
async fn a_shed_request_is_logged_at_warn_naming_the_transport_and_none_is_lost() {
    let lines = ShedLines::default();
    let _logged = tracing::subscriber::set_default(lines.clone());
    let keeper = Keeper::default();
    let gate = Gate::default();
    let app = App::builder(Root(gate.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()).max_inflight(Count::Max(1)))
        .listen()
        .await
        .expect("the app listens");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let svc = keeper.service();
    drop(keeper);

    let held = tokio::spawn(svc.call(get("/held")));
    tokio::time::timeout(PATIENCE, async {
        while gate.held.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the held request reached its handler");
    for _ in 0..3 {
        let (status, _) = written(tokio::time::timeout(PATIENCE, svc.call(get("/held"))).await.expect("the service answered")).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "a request over the bound was answered");
    }
    assert_eq!(lines.written(), vec![("http".to_owned(), 1)], "the first refusal's line, the next two counted");

    gate.open.notify_waiters();
    let _ = tokio::time::timeout(PATIENCE, held).await.expect("the held request was answered");
    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
    drop(svc);
    assert_eq!(lines.written(), vec![("http".to_owned(), 1), ("http".to_owned(), 2)], "the two counted refusals, written once the server drops");
}
