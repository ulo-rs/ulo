//! A link declaring `delivers_requests_once: false` hands each request to every server instance
//! listening on its pattern, so `Server::prepare` logs one `warn` per server that mounts a request
//! handler, naming how many request patterns it mounts and the link's reason, and refuses
//! nothing. A server mounting only events on such a link, and one on a link delivering once, log
//! nothing. The links are TCP with the capability changed, so no broker is needed.

use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::{App, AppHandle, BoundAddr, BoxError, Module, ModuleDef, ModuleIdentity, Runtime, Signal, injectable, routes};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Capabilities, Link, Outbound, Pattern, Payload};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;
use ulo_transport::{Classify, Count, ErrorKind};

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

#[injectable]
struct Mixed;

#[routes]
impl Mixed {
    #[ulo_rpc::message("fan.one")]
    async fn one(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0)
    }

    #[ulo_rpc::message("fan.two")]
    async fn two(&self, n: Payload<u32>) -> Result<u32, Never> {
        Ok(n.0)
    }

    #[ulo_rpc::event("fan.seen")]
    async fn seen(&self, _n: Payload<u32>) -> Result<(), Never> {
        Ok(())
    }
}

#[injectable]
struct EventsOnly;

#[routes]
impl EventsOnly {
    #[ulo_rpc::event("fan.only")]
    async fn only(&self, _n: Payload<u32>) -> Result<(), Never> {
        Ok(())
    }
}

struct MixedRoot;

impl Module for MixedRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Mixed>();
    }
}

struct EventsRoot;

impl Module for EventsRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<EventsOnly>();
    }
}

/// The TCP link declaring it hands each request to every instance, for `reason`.
struct Fanned {
    tcp: Tcp,
    reason: &'static str,
}

/// The TCP link under a name of its own, so its server's events tell from `Fanned`'s.
struct Once {
    tcp: Tcp,
}

macro_rules! tcp_link {
    ($link:ty, $name:literal, |$this:ident| $capabilities:expr) => {
        impl Link for $link {
            const NAME: &'static str = $name;

            fn capabilities(&self) -> Capabilities {
                let $this = self;
                $capabilities
            }

            async fn prepare(&mut self, app: &AppHandle) -> Result<(), BoxError> {
                self.tcp.prepare(app).await
            }

            fn usable(&self) -> Result<(), BoxError> {
                self.tcp.usable()
            }

            fn max_inflight(&mut self, calls: Count) {
                self.tcp.max_inflight(calls);
            }

            async fn listen(&self, patterns: &[Pattern]) -> Result<Inbound, BoxError> {
                self.tcp.listen(patterns).await
            }

            async fn connect(&self, runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
                self.tcp.connect(runtime).await
            }

            async fn drain(&self) {
                self.tcp.drain().await;
            }

            async fn close(&self) -> Result<(), BoxError> {
                self.tcp.close().await
            }

            fn bound(&self) -> Vec<BoundAddr> {
                self.tcp.bound()
            }
        }
    };
}

tcp_link!(Fanned, "fanned", |this| this.tcp.capabilities().fans_out_requests(this.reason));
tcp_link!(Once, "once", |this| this.tcp.capabilities());

/// Binds `link` under `root` and closes the app at once.
async fn bind_and_close<L: Link>(root: impl Module, link: L) {
    let app = App::builder(root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(link))
        .listen()
        .await
        .map_err(Box::new)
        .expect("the server binds: the capability refuses nothing");
    app.serve(async { Signal::new("test") }).await.expect("the app shuts down");
}

#[tokio::test]
async fn a_server_on_a_link_fanning_out_requests_warns_once_naming_its_request_patterns() {
    const REASON: &str = "the test link fans out its requests";
    let lines = capture();
    bind_and_close(MixedRoot, Fanned { tcp: Tcp::new("127.0.0.1:0"), reason: REASON }).await;
    let warned = lines.matching(&format!("reason={REASON:?}"));
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].contains("link=\"fanned\"") && warned[0].contains("request_patterns=2"), "{warned:?}");
    assert!(warned[0].contains("every server instance listening on its pattern") && warned[0].contains("run one server instance"), "{warned:?}");
}

#[tokio::test]
async fn a_server_mounting_only_events_on_a_link_fanning_out_requests_logs_nothing() {
    const REASON: &str = "the events-only link fans out its requests";
    let lines = capture();
    bind_and_close(EventsRoot, Fanned { tcp: Tcp::new("127.0.0.1:0"), reason: REASON }).await;
    let warned = lines.matching(&format!("reason={REASON:?}"));
    assert!(warned.is_empty(), "{warned:?}");
}

#[tokio::test]
async fn a_server_on_a_link_delivering_requests_once_logs_nothing() {
    let lines = capture();
    let link = Once { tcp: Tcp::new("127.0.0.1:0") };
    let capabilities = link.capabilities();
    assert!(capabilities.delivers_requests_once && capabilities.fan_out_reason.is_none(), "{capabilities:?}");
    bind_and_close(MixedRoot, link).await;
    let warned = lines.matching("link=\"once\"");
    assert!(warned.is_empty(), "{warned:?}");
}

fn capture() -> Capture {
    static CAPTURE: OnceLock<Capture> = OnceLock::new();
    CAPTURE
        .get_or_init(|| {
            let capture = Capture::default();
            tracing::subscriber::set_global_default(capture.clone()).expect("no other subscriber is installed");
            capture
        })
        .clone()
}

/// Every `warn` event, as its message followed by `name=value` for each other field.
#[derive(Clone, Default)]
struct Capture {
    lines: Arc<Mutex<Vec<String>>>,
}

impl Capture {
    fn matching(&self, field: &str) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().filter(|line| line.contains(field)).cloned().collect()
    }
}

impl tracing::Subscriber for Capture {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        *metadata.level() <= Level::WARN
    }

    fn new_span(&self, _span: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &TraceEvent<'_>) {
        let mut line = Line::default();
        event.record(&mut line);
        self.lines.lock().unwrap_or_else(PoisonError::into_inner).push(format!("{}{}", line.message, line.fields));
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
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields.push_str(&format!(" {}={value:?}", field.name()));
        }
    }
}
