//! An RPC server over a link binding no address names itself in its shed lines' `server` field by
//! the link's name and the endpoint the link answers, which a broker link redacts as the five
//! broker crates' unit tests pin (transports DESIGN §2.8). The link is scripted, redacting its
//! URL as they do, so a broker's URL reaches the server with no broker behind it.
//!
//! One test in this binary: a `tracing` callsite first reached on a thread with no subscriber,
//! while another thread installs one, can keep the interest it computed without it.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::stream;
use tokio::sync::{Notify, mpsc};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata};
use ulo::{App, BoxError, Module, ModuleDef, ModuleIdentity, Runtime, Signal, controller, handlers};
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Codec, Data, Delivery, DeliveryMode, Frame, Link, Outbound, ReplyPath, Subscription};
use ulo_transport::{Classify, Count, ErrorKind};

const PATIENCE: Duration = Duration::from_secs(10);

/// What the link was configured with: a password in the userinfo and a token in the query.
const ENDPOINT: &str = "scripted://user:pw-in-userinfo@broker:4222/vhost?token=pw-in-query";

#[derive(Debug)]
struct Never;

impl std::fmt::Display for Never {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("never")
    }
}

impl std::error::Error for Never {}

impl Classify for Never {
    fn classify(&self) -> ErrorKind {
        ErrorKind::Internal
    }
}

static HELD: Notify = Notify::const_new();
static RELEASE: Notify = Notify::const_new();

#[controller]
struct Holder;

#[handlers]
impl Holder {
    #[ulo_rpc::message("shed.hold")]
    async fn hold(&self) -> Result<u32, Never> {
        HELD.notify_one();
        RELEASE.notified().await;
        Ok(1)
    }
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

/// A link whose inbound stream the test feeds, configured with [`ENDPOINT`].
struct Scripted(Mutex<Option<Inbound>>);

impl Link for Scripted {
    const NAME: &'static str = "scripted";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).codec(Codec::Json)
    }

    fn endpoint(&self) -> Option<String> {
        Some(ulo::secrets::setting_url(ENDPOINT))
    }

    async fn listen(&self, _patterns: &[Subscription]) -> Result<Inbound, BoxError> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or_else(|| "the scripted link listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        Err("the scripted link has no client side".into())
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
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

/// A request for the held handler, and the receiver of the frames the server writes back.
fn request(id: u64) -> (Delivery, mpsc::UnboundedReceiver<Frame>) {
    let (frames, written) = mpsc::unbounded_channel();
    let reply = ReplyPath::new(move |frame: Frame| {
        let sent = frames.send(frame).map_err(BoxError::from);
        Box::pin(async move { sent })
    });
    let frame = Frame::req(id, "shed.hold", CallHeaders::new(), Data::new(b"null".as_slice()));
    (Delivery { frame, reply: Some(reply), ack: Ack::new(|_| {}) }, written)
}

#[tokio::test(flavor = "current_thread")]
async fn a_broker_links_shed_line_names_the_link_and_its_endpoint_without_credentials() {
    let lines = ShedLines::default();
    let _logged = tracing::subscriber::set_default(lines.clone());
    let (deliveries, mut receiver) = mpsc::unbounded_channel::<Delivery>();
    let inbound: Inbound = Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx)));
    let app = App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Scripted(Mutex::new(Some(inbound)))).max_inflight(Count::Max(1)))
        .listen()
        .await
        .expect("the server binds");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });

    let (held, _held_frames) = request(1);
    deliveries.send(held).expect("the server reads its link");
    tokio::time::timeout(PATIENCE, HELD.notified()).await.expect("the first call reached its handler");
    let (refused, mut refused_frames) = request(2);
    deliveries.send(refused).expect("the server reads its link");
    let answer = tokio::time::timeout(PATIENCE, refused_frames.recv()).await.expect("the refusal in time");
    assert!(matches!(answer, Some(Frame::Err { id: 2, .. })), "the call over the bound was answered {answer:?}");
    assert_eq!(lines.written(), vec![("scripted scripted://broker:4222/vhost".to_owned(), 1)]);

    RELEASE.notify_one();
    drop(deliveries);
    let _ = tokio::time::timeout(PATIENCE, handle.close(Signal::new("test"))).await.expect("close finished in time");
    let _ = tokio::time::timeout(PATIENCE, serving).await.expect("serve ended in time");
}
