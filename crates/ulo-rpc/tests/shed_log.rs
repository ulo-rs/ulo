//! A call shed over `max_inflight` is counted into the server's `ShedLog` (transports DESIGN §2.8),
//! a request refused with its `RetryAfter` and an event left unacknowledged alike: the first
//! refusal writes a `warn` naming the transport, and the server by its link and bound address, at
//! once, and the refusals after it within the interval are written when the server drops with the
//! app, none lost.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::Notify;
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal, controller, handlers};
use ulo_rpc::{Payload, RpcClient};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, Count, ErrorKind};

const PATIENCE: Duration = Duration::from_secs(10);

/// The `debug` line the dispatcher writes for an event it leaves unacknowledged.
const EVENT_LEFT: &str = "an event over the in-flight limit was left unacknowledged";

#[derive(Debug)]
struct Never;

impl fmt::Display for Never {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("never raised")
    }
}

impl Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

static RELEASE: Notify = Notify::const_new();
static HELD: Notify = Notify::const_new();

#[controller]
struct Holder;

#[handlers]
impl Holder {
    #[ulo_rpc::message("shed.hold")]
    async fn hold(&self, n: Payload<u32>) -> Result<u32, Never> {
        HELD.notify_one();
        RELEASE.notified().await;
        Ok(n.0)
    }

    #[ulo_rpc::event("shed.event")]
    async fn event(&self, _n: Payload<u32>) {}
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Holder>();
    }
}

/// The `ShedLog` lines written at `warn`, as their `ulo.transport`, `server` and `shed` fields,
/// and every message written at `debug`.
#[derive(Clone, Default)]
struct Lines {
    shed: Arc<Mutex<Vec<(String, String, u64)>>>,
    debug: Arc<Mutex<Vec<String>>>,
}

impl Lines {
    fn shed(&self) -> Vec<(String, String, u64)> {
        self.shed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn debugged(&self, message: &str) -> bool {
        self.debug.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|line| line == message)
    }
}

#[derive(Default)]
struct Line {
    message: String,
    transport: String,
    server: String,
    shed: Option<u64>,
}

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "ulo.transport" => self.transport = value.to_owned(),
            "server" => self.server = value.to_owned(),
            _ => {}
        }
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        if field.name() == "shed" {
            self.shed = Some(value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        }
    }
}

impl tracing::Subscriber for Lines {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event() && matches!(*metadata.level(), Level::WARN | Level::DEBUG)
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        match (*event.metadata().level(), line.shed) {
            (Level::WARN, Some(shed)) => self.shed.lock().unwrap_or_else(PoisonError::into_inner).push((line.transport, line.server, shed)),
            (Level::DEBUG, _) => self.debug.lock().unwrap_or_else(PoisonError::into_inner).push(line.message),
            _ => {}
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[tokio::test(flavor = "current_thread")]
async fn a_shed_request_and_a_shed_event_are_logged_at_warn_naming_the_transport_and_none_is_lost() {
    let lines = Lines::default();
    let _logged = tracing::subscriber::set_default(lines.clone());
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")).max_inflight(Count::Max(1)))
        .listen()
        .await
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let line = |shed| ("rpc".to_owned(), format!("tcp {addr}"), shed);
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");

    let held = tokio::spawn({
        let client = client.clone();
        async move { client.request::<u32, u32>("shed.hold", &1).await }
    });
    tokio::time::timeout(PATIENCE, HELD.notified()).await.expect("the first call reached its handler");
    for n in 2..4 {
        let refused = tokio::time::timeout(PATIENCE, client.request::<u32, u32>("shed.hold", &n)).await.expect("a refusal in time");
        assert_eq!(refused.map_err(|error| error.kind()), Err(ErrorKind::Unavailable));
    }
    tokio::time::timeout(PATIENCE, client.emit("shed.event", &4u32)).await.expect("the event was sent in time").expect("the event was sent");
    tokio::time::timeout(PATIENCE, async {
        while !lines.debugged(EVENT_LEFT) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the event over the bound reached the dispatcher");
    assert_eq!(lines.shed(), vec![line(1)], "the first refusal's line, the request and the event after it counted");

    RELEASE.notify_one();
    let answered = tokio::time::timeout(PATIENCE, held).await.expect("the held call answered in time").expect("the held call's task");
    assert_eq!(answered.expect("the held call is answered"), 1);
    drop(client);
    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
    assert_eq!(lines.shed(), vec![line(1), line(2)], "the two counted refusals, written once the server drops");
}
