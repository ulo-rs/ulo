//! A handler declared `Unary` that answers with a stream is refused at the reply: the
//! caller gets `Internal` with the reason `shape_mismatch` and the server logs one `warn` naming
//! the handler, instead of the reply streaming to a caller who asked for one payload.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::{StreamExt, stream};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Level, Metadata};
use ulo::app::Bound as Listening;
use ulo::scope::Auto;
use ulo::{
    App, AppHandle, BoxError, Construct, ConstructError, Controller, Dependencies, HandlerSpec, Module, ModuleDef, ModuleIdentity, Mount,
    Resolver, Shape, Signal,
};
use ulo_rpc::{CallError, ErrorKind, Reply, Rpc, RpcClient, RpcCx, RpcHandler};
use ulo_rpc_tcp::Tcp;
use ulo_runtime_tokio::Tokio;

const PATIENCE: Duration = Duration::from_secs(10);

struct Shapes;

impl Construct for Shapes {
    type Scope = Auto;

    fn dependencies(_: &mut Dependencies) {}

    async fn construct(_: &Resolver<'_>) -> Result<Self, ConstructError> {
        Ok(Shapes)
    }
}

async fn stream_of_two(cx: RpcCx) -> Result<Reply, BoxError> {
    Ok(cx.reply_stream(stream::iter([1u32, 2].map(Ok::<_, CallError>))))
}

async fn one_payload(cx: RpcCx) -> Result<Reply, BoxError> {
    Ok(cx.reply(&7u32)?)
}

impl Controller for Shapes {
    fn mount(m: &mut Mount<'_>) {
        m.handler(HandlerSpec::<Rpc, _>::new("unary_streams", RpcHandler::message(stream_of_two)).route("s.unary_streams"));
        m.handler(HandlerSpec::<Rpc, _>::new("unary_matches", RpcHandler::message(one_payload)).route("s.unary_matches"));
        m.handler(
            HandlerSpec::<Rpc, _>::new("streaming_matches", RpcHandler::message(stream_of_two))
                .route("s.streaming_matches")
                .shape(Shape::ServerStreaming),
        );
        m.handler(
            HandlerSpec::<Rpc, _>::new("client_streaming_streams", RpcHandler::message(stream_of_two))
                .route("s.client_streaming_streams")
                .shape(Shape::ClientStreaming),
        );
        m.handler(HandlerSpec::<Rpc, _>::new("bidi_matches", RpcHandler::message(stream_of_two)).route("s.bidi_matches").shape(Shape::Bidi));
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Shapes>();
    }
}

struct Running {
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    client: RpcClient,
}

async fn start() -> Running {
    let app: App<Listening> = App::builder(Root)
        .runtime(Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects")
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .map_err(Box::new)
        .expect("the server binds");
    let addr = app.addresses().first().map(|bound| bound.addr).expect("the server's address");
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let client = RpcClient::new(Tcp::new(addr.to_string()), Arc::new(Tokio::current())).expect("a client on the bound address");
    Running { handle, serving, client }
}

impl Running {
    async fn stop(self) {
        let _ = tokio::time::timeout(PATIENCE, self.handle.close(Signal::new("test"))).await.expect("close finished in time");
        let _ = tokio::time::timeout(PATIENCE, self.serving).await.expect("serve ended in time");
    }
}

#[tokio::test]
async fn a_unary_handler_replying_a_stream_is_refused_at_the_reply() {
    let lines = capture();
    let app = start().await;
    let error = tokio::time::timeout(PATIENCE, app.client.request::<_, ()>("s.unary_streams", &0u32))
        .await
        .expect("in time")
        .expect_err("the mismatched reply is refused");
    assert_eq!(error.kind(), ErrorKind::Internal, "{error}");
    assert_eq!(error.reason(), Some("shape_mismatch"), "{error}");
    assert!(error.message().contains("`unary_streams` is declared Unary and answered with a stream"), "{error}");
    let warned = lines.matching("unary_streams");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].contains("pattern=\"s.unary_streams\"") && warned[0].contains("declared=Unary"), "{warned:?}");
    app.stop().await;
}

#[tokio::test]
async fn a_client_streaming_handler_replying_a_stream_is_refused_at_the_reply() {
    let lines = capture();
    let app = start().await;
    let error = tokio::time::timeout(PATIENCE, app.client.send_stream::<_, ()>("s.client_streaming_streams", stream::iter([1u32])))
        .await
        .expect("in time")
        .expect_err("the mismatched reply is refused");
    assert_eq!((error.kind(), error.reason()), (ErrorKind::Internal, Some("shape_mismatch")), "{error}");
    assert!(error.message().contains("`client_streaming_streams` is declared ClientStreaming and answered with a stream"), "{error}");
    assert_eq!(lines.matching("client_streaming_streams").len(), 1, "{:?}", lines.matching("client_streaming_streams"));
    app.stop().await;
}

#[tokio::test]
async fn a_reply_in_the_declared_shape_is_written_and_logs_nothing() {
    let lines = capture();
    let app = start().await;
    let one = tokio::time::timeout(PATIENCE, app.client.request::<_, u32>("s.unary_matches", &0u32)).await.expect("in time");
    assert_eq!(one.expect("an answer"), 7);
    let items = tokio::time::timeout(PATIENCE, async {
        let items = app.client.stream::<_, u32>("s.streaming_matches", &0u32).await.expect("the stream opens");
        items.map(|item| item.expect("an item")).collect::<Vec<u32>>().await
    })
    .await
    .expect("in time");
    assert_eq!(items, [1, 2]);
    let duplexed = tokio::time::timeout(PATIENCE, async {
        let items = app.client.duplex::<_, u32>("s.bidi_matches", stream::iter([0u32])).await.expect("the duplex opens");
        items.map(|item| item.expect("an item")).collect::<Vec<u32>>().await
    })
    .await
    .expect("in time");
    assert_eq!(duplexed, [1, 2]);
    assert!(lines.matching("matches").is_empty(), "{:?}", lines.matching("matches"));
    app.stop().await;
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
    fn matching(&self, handler: &str) -> Vec<String> {
        let lines = self.lines.lock().unwrap_or_else(PoisonError::into_inner);
        lines.iter().filter(|line| line.contains(&format!("handler=Some(\"{handler}\")")) || line.contains(&format!("handler=\"{handler}\""))).cloned().collect()
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
