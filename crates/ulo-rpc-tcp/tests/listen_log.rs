//! A server writes the address it bound at `info` once it listens, beside the endpoint as
//! written, so a deployer sees which of a host name's addresses it took: `localhost` resolves to
//! more than one here, and the server binds the first that binds.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{App, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_rpc::Server;
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, _m: &mut ModuleDef<'_>) {}
}

/// Every `info` event carrying a `link` field, as its `link`, `endpoint` and `address`.
#[derive(Clone, Default)]
struct Lines(Arc<Mutex<Vec<(String, String, String)>>>);

impl Lines {
    fn written(&self) -> Vec<(String, String, String)> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[derive(Default)]
struct Line {
    link: Option<String>,
    endpoint: String,
    address: String,
}

impl Visit for Line {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "link" {
            self.link = Some(value.to_owned());
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        match field.name() {
            "endpoint" => self.endpoint = format!("{value:?}"),
            "address" => self.address = format!("{value:?}"),
            _ => {}
        }
    }
}

impl tracing::Subscriber for Lines {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.is_event() && *metadata.level() == Level::INFO
    }
    fn new_span(&self, _: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }
    fn record(&self, _: &Id, _: &Record<'_>) {}
    fn record_follows_from(&self, _: &Id, _: &Id) {}
    fn event(&self, event: &Event<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        if let Some(link) = line.link {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).push((link, line.endpoint, line.address));
        }
    }
    fn enter(&self, _: &Id) {}
    fn exit(&self, _: &Id) {}
}

#[tokio::test(flavor = "current_thread")]
async fn a_server_on_a_host_name_logs_the_address_it_bound() {
    let lines = Lines::default();
    let _logged = tracing::subscriber::set_default(lines.clone());
    let app = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(Server::new(Tcp::new("localhost:0")))
        .listen()
        .await
        .unwrap_or_else(|refused| panic!("a server on `localhost:0` did not listen: {refused}"));
    let bound = match app.addresses().as_slice() {
        [bound] => bound.addr,
        other => panic!("expected one bound address, got {other:?}"),
    };
    assert_eq!(lines.written(), vec![("tcp".to_owned(), "localhost:0".to_owned(), bound.to_string())], "the lines written at `info`");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let _ = handle.close(Signal::new("listen_log")).await;
    let _ = serving.await;
}
