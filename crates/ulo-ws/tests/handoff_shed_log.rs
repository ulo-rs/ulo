//! A connection refused over a gateway's `max_connections` on the HTTP server's port is counted
//! into a `ShedLog` (transports DESIGN §2.8, §4.2) whose `server` field is the address the upgrade
//! arrived on, the HTTP server's bound address.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

mod support;

use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{AppBuilder, Module, ModuleDef, ModuleIdentity, controller, handlers};
use ulo_ws::{Payload, Ws, WsModule};

use support::{Running, close_frame, hang_up};

#[controller]
struct One;

#[handlers]
#[ulo_ws::gateway(path = "/one", max_connections = 1)]
impl One {
    #[ulo_ws::message("echo")]
    fn echo(&self, text: Payload<String>) -> String {
        text.0
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.controller::<One>();
    }
}

/// Every `ShedLog` line written at `warn`, as its `ulo.transport`, `server` and `shed` fields.
#[derive(Clone, Default)]
struct ShedLines(Arc<Mutex<Vec<(String, String, u64)>>>);

impl ShedLines {
    fn written(&self) -> Vec<(String, String, u64)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Default)]
struct ShedLine {
    transport: String,
    server: String,
    shed: Option<u64>,
}

impl Visit for ShedLine {
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
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((line.transport, line.server, shed));
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[tokio::test(flavor = "current_thread")]
async fn a_connection_refused_on_the_http_port_names_the_http_servers_address() {
    let lines = ShedLines::default();
    let _logged = tracing::subscriber::set_default(lines.clone());
    let app = Running::start_configured(Root, ulo_http_hyper::Server::new("127.0.0.1:0"), ulo_runtime_tokio::Tokio::current(), AppBuilder::unserved::<Ws>)
        .await;
    let line = |shed| ("ws_connect".to_owned(), app.addr.to_string(), shed);
    let admitted = app.connect("/one", &[]).await;
    for _ in 0..2 {
        let mut refused = app.connect("/one", &[]).await;
        assert_eq!(close_frame(&mut refused).await.map(|(code, _)| code), Some(1013), "a connection over the bound was admitted");
        hang_up(refused).await;
    }
    assert_eq!(lines.written(), vec![line(1)], "the first refusal's line, the next counted");

    let (first, residue) = (line(1), line(1));
    hang_up(admitted).await;
    app.stop().await;
    assert_eq!(lines.written(), vec![first, residue], "the counted refusal, written once the hand-off's table drops");
}
