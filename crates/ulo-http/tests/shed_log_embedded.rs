//! A request an embedding sheds over `max_inflight` is counted into its `ShedLog` (transports
//! DESIGN §2.8, §3.8), whose `server` field names the host and the mount prefix, the embedding
//! binding no address of its own.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{App, Dep, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_http::embed::{Embed, EmbedLimits, Embedded, Service};
use ulo_http::{ConnInfo, HttpBody, Request, StatusCode};
use ulo_transport::Count;

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

struct Host;

impl Embed for Host {
    const NAME: &'static str = "test-host";

    type HostRequest<'r> = http::request::Parts;

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
    }
}

/// Every `ShedLog` line written at `warn`, as its `server` and `shed` fields.
#[derive(Clone, Default)]
struct ShedLines(Arc<Mutex<Vec<(String, u64)>>>);

impl ShedLines {
    fn written(&self) -> Vec<(String, u64)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Default)]
struct ShedLine {
    server: String,
    shed: Option<u64>,
}

impl Visit for ShedLine {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "server" {
            self.server = value.to_owned();
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
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((line.server, shed));
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

fn get() -> Request {
    let (head, ()) = http::Request::builder().uri("/held").body(()).expect("the request builds").into_parts();
    Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None }
}

async fn call(service: &Service<Host>) -> StatusCode {
    let req = get();
    let head = req.head.clone();
    service.respond(&head, req).await.status()
}

/// One request held, then one shed, on an embedding built by `server`, the host handing the app
/// each path with the mount prefix removed; the app dropped.
async fn sheds_one(server: Embedded<Host>) {
    let gate = Gate::default();
    let handle = server.handle();
    let app = App::builder(Root(gate.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(server.max_inflight(Count::Max(1)))
        .listen()
        .await
        .expect("the app listens");
    let service = handle.service();
    let held = tokio::spawn({
        let service = service.clone();
        async move { call(&service).await }
    });
    tokio::time::timeout(PATIENCE, async {
        while gate.held.load(Ordering::SeqCst) < 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the held request reached its handler");
    let refused = tokio::time::timeout(PATIENCE, call(&service)).await.expect("the service answered");
    assert_eq!(refused, StatusCode::SERVICE_UNAVAILABLE, "a request over the bound was answered");
    gate.open.notify_waiters();
    let _ = tokio::time::timeout(PATIENCE, held).await.expect("the held request was answered");
    drop(app);
}

#[tokio::test(flavor = "current_thread")]
async fn an_embeddings_shed_line_names_the_host_and_the_mount_prefix() {
    let lines = ShedLines::default();
    let _logged = tracing::subscriber::set_default(lines.clone());
    sheds_one(Embedded::<Host>::new()).await;
    sheds_one(Embedded::<Host>::new().nested_at("/api")).await;
    assert_eq!(lines.written(), vec![("test-host embedding".to_owned(), 1), ("test-host embedding at /api".to_owned(), 1)]);
}
