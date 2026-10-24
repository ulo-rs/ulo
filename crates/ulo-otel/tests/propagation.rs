//! W3C Trace Context across the transports, end to end (integrations DESIGN §6.1 to §6.3, decisions
//! 76 and 77; SWITCH_INVENTORY G24): a `traceparent` an HTTP, RPC or gRPC call carries becomes the
//! execution's `Received` context, a call made `.within` that execution forwards its bytes
//! unchanged, a context originated here writes a fresh `traceparent`, and a header the client
//! writes itself is refused.
//!
//! The home is `ulo-otel` because it is the one crate that reaches every transport and holds the
//! real `TraceSource`; the pass-through tests bind none. HTTP calls go through `AppService` as a
//! backend would, RPC frames through a scripted link, gRPC through a tonic channel on loopback port 0.
//! Assertions are on the bytes of the headers and the `TraceOrigin`, never on message text. Every
//! wait is under a real-clock deadline.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use futures_util::stream;
use tokio::sync::{Notify, mpsc};
use tracing::Span;
use ulo::{
    App, AppHandle, BoxError, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, TraceContext, TraceMode, TraceOrigin,
    TraceSource, injectable, routes,
};
use ulo_http::{AppService, Backend, BackendLimits, ConnInfo, HttpBody, HttpConfig, Request, StatusCode};
use ulo_net::BoundListener;
use ulo_net::rustls::ServerConfig;
use ulo_rpc::link::Inbound;
use ulo_rpc::{Ack, CallHeaders, Capabilities, Data, Delivery, DeliveryMode, Frame, Link, Outbound, Pattern, Payload, ReplyPath, RpcClient};
use ulo_transport::ErrorKind;

const PATIENCE: Duration = Duration::from_secs(10);

const TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
const TRACE_ID: [u8; 16] = [0x4b, 0xf9, 0x2f, 0x35, 0x77, 0xb3, 0x4d, 0xa6, 0xa3, 0xce, 0x92, 0x9d, 0x0e, 0x0e, 0x47, 0x36];
const SPAN_ID: [u8; 8] = [0x00, 0xf0, 0x67, 0xaa, 0x0b, 0xa9, 0x02, 0xb7];
const TRACESTATE: &str = "vendor1=opaque,congo=t61rcWkgMzE";
/// A version this implementation does not know, with a field after the flags and Level 2's
/// `random` flag set beside `sampled`: valid to parse and to pass through, not to originate.
const FUTURE: &str = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03-future-field";

async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(PATIENCE, fut).await.unwrap_or_else(|_| panic!("{what} did not happen within {PATIENCE:?}"))
}

/// A refusal an outgoing call answered: its kind and `reason`; `None` for a call that was sent.
type Refusal = Option<(ErrorKind, Option<String>)>;

/// `ulo_grpc::outgoing`'s answer: the metadata written, or the status code and `ErrorInfo` reason.
type GrpcAnswer = Result<Vec<(String, String)>, (tonic::Code, Option<String>)>;

/// What the handlers and the sources recorded.
#[derive(Clone, Default)]
struct Rec(Arc<RecInner>);

#[derive(Default)]
struct RecInner {
    /// The execution's context as a handler read it, by transport label.
    seen: Mutex<Vec<(&'static str, Option<TraceContext>)>>,
    /// What an outgoing call made `.within` an execution answered, by label: the kind and the
    /// `reason` of a refusal, or `None` for a call that was sent.
    emitted: Mutex<Vec<(&'static str, Refusal)>>,
    /// `ulo_grpc::outgoing`'s answer, by label: the metadata written, or the refusal.
    grpc: Mutex<Vec<(&'static str, GrpcAnswer)>>,
    /// The inbound context a `TraceSource` was handed, one entry per call served.
    inbound: Mutex<Vec<Option<TraceContext>>>,
    gate: Notify,
}

impl Rec {
    fn saw(&self, label: &'static str, exec: &ExecutionRef) {
        self.0.seen.lock().unwrap_or_else(PoisonError::into_inner).push((label, exec.trace().cloned()));
        self.0.gate.notify_waiters();
    }

    fn emitted(&self, label: &'static str, result: Result<(), ulo_rpc::RpcError>) {
        let outcome = result.err().map(|error| (error.kind(), error.reason().map(str::to_owned)));
        self.0.emitted.lock().unwrap_or_else(PoisonError::into_inner).push((label, outcome));
    }

    fn seen(&self) -> Vec<(&'static str, Option<TraceContext>)> {
        self.0.seen.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn last_seen(&self) -> Option<TraceContext> {
        self.seen().pop().and_then(|(_, context)| context)
    }

    fn emitted_outcomes(&self) -> Vec<(&'static str, Refusal)> {
        self.0.emitted.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn inbound(&self) -> Vec<Option<TraceContext>> {
        self.0.inbound.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    async fn until_seen(&self, n: usize) {
        within("the handler to record", async {
            loop {
                let notified = self.0.gate.notified();
                if self.seen().len() >= n {
                    return;
                }
                let _ = tokio::time::timeout(Duration::from_millis(20), notified).await;
            }
        })
        .await;
    }
}

/// The RPC link of the app: its server side is fed by the test, its client side records every
/// frame the app sends and never replies.
#[derive(Clone)]
struct Wire {
    inbound: Arc<Mutex<Option<Inbound>>>,
    sent: Arc<Mutex<Vec<Frame>>>,
}

impl Wire {
    fn new() -> (Wire, mpsc::UnboundedSender<Delivery>) {
        let (deliveries, mut receiver) = mpsc::unbounded_channel::<Delivery>();
        let inbound: Inbound = Box::pin(stream::poll_fn(move |cx| receiver.poll_recv(cx)));
        (Wire { inbound: Arc::new(Mutex::new(Some(inbound))), sent: Arc::default() }, deliveries)
    }

    fn sent(&self) -> Vec<Frame> {
        self.sent.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// The headers of the `n`th frame the app sent, which must be an event or an opening frame.
    fn headers_of(&self, n: usize) -> CallHeaders {
        match self.sent().get(n) {
            Some(Frame::Evt { headers, .. } | Frame::Req { headers, .. }) => headers.clone(),
            other => panic!("expected the app's frame {n} to be an event or a request, found {other:?}"),
        }
    }
}

impl Link for Wire {
    const NAME: &'static str = "wire";

    fn capabilities(&self) -> Capabilities {
        Capabilities::new(DeliveryMode::Addressed).miss_signal(true)
    }

    async fn listen(&self, _patterns: &[Pattern]) -> Result<Inbound, BoxError> {
        self.inbound.lock().unwrap_or_else(PoisonError::into_inner).take().ok_or_else(|| "the wire listened twice".into())
    }

    async fn connect(&self, _runtime: &Arc<dyn Runtime>) -> Result<Outbound, BoxError> {
        let sent = Arc::clone(&self.sent);
        Ok(Outbound {
            send: Box::new(move |_pattern, frame, _reply_to| {
                sent.lock().unwrap_or_else(PoisonError::into_inner).push(frame);
                Box::pin(async { Ok(()) })
            }),
            replies: Box::pin(stream::pending()),
        })
    }

    async fn drain(&self) {}

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// The app's RPC client, over the wire.
struct Out(RpcClient);

/// The `TraceSource` of the tests that originate a context: it records what it was handed and
/// answers the inbound trace under a span of its own, flags and state kept as they came.
struct Fixed {
    rec: Rec,
}

const OWN_SPAN: [u8; 8] = [0xaa; 8];

impl TraceSource for Fixed {
    fn start(&self, _span: &Span, inbound: Option<&TraceContext>) -> TraceContext {
        self.rec.0.inbound.lock().unwrap_or_else(PoisonError::into_inner).push(inbound.cloned());
        let trace_id = inbound.map_or([0x11; 16], TraceContext::trace_id);
        let flags = inbound.map_or(0x01, TraceContext::flags);
        let state = inbound.map(|context| context.state().clone()).unwrap_or_default();
        TraceContext::new(trace_id, OWN_SPAN, flags, state).expect("a valid context")
    }
}

/// Binds a source as `dyn TraceSource`, visible from the root module.
struct Sourced(Arc<dyn TraceSource>);

struct Shared(Arc<dyn TraceSource>);

impl TraceSource for Shared {
    fn start(&self, span: &Span, inbound: Option<&TraceContext>) -> TraceContext {
        self.0.start(span, inbound)
    }
}

impl Module for Sourced {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.global();
        m.value(Shared(Arc::clone(&self.0))).also_as::<dyn TraceSource>(|source| source);
        m.export::<dyn TraceSource>();
    }
}

/// The HTTP side: each route records the context its execution carries, and some make a call
/// `.within` that execution.
#[injectable]
struct Edge {
    rec: Dep<Rec>,
    out: Dep<Out>,
}

const CALLER_TRACEPARENT: &str = "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-01";

#[routes]
impl Edge {
    #[ulo_http::get("/seen")]
    async fn seen(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        "ok"
    }

    /// An event `.within` the execution.
    #[ulo_http::get("/forward")]
    async fn forward(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        self.rec.emitted("forward", self.out.0.emit("out.evt", &1u32).within(&exec).await);
        "ok"
    }

    /// The same, the caller setting `traceparent` itself.
    #[ulo_http::get("/forward-traceparent")]
    async fn forward_traceparent(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        self.rec.emitted("traceparent", self.out.0.emit("out.evt", &1u32).header("traceparent", CALLER_TRACEPARENT).within(&exec).await);
        "ok"
    }

    /// The same, the caller adding a `tracestate` entry.
    #[ulo_http::get("/forward-tracestate")]
    async fn forward_tracestate(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        self.rec.emitted("tracestate", self.out.0.emit("out.evt", &1u32).header("tracestate", "mine=1").within(&exec).await);
        "ok"
    }

    /// An event with the headers set by the caller and no `.within`: ordinary headers.
    #[ulo_http::get("/forward-plain")]
    async fn forward_plain(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        let emit = self.out.0.emit("out.evt", &1u32).header("traceparent", CALLER_TRACEPARENT).header("tracestate", "mine=1");
        self.rec.emitted("plain", emit.await);
        "ok"
    }

    /// `ulo_grpc::outgoing` over an empty request.
    #[ulo_http::get("/grpc")]
    async fn grpc(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        self.grpc_with("grpc", &exec, &[]);
        "ok"
    }

    #[ulo_http::get("/grpc-traceparent")]
    async fn grpc_traceparent(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        self.grpc_with("grpc-traceparent", &exec, &[("traceparent", CALLER_TRACEPARENT)]);
        "ok"
    }

    #[ulo_http::get("/grpc-tracestate")]
    async fn grpc_tracestate(&self, exec: ExecutionRef) -> &'static str {
        self.rec.saw("http", &exec);
        self.grpc_with("grpc-tracestate", &exec, &[("tracestate", "mine=1")]);
        "ok"
    }
}

impl Edge {
    fn grpc_with(&self, label: &'static str, exec: &ExecutionRef, metadata: &[(&'static str, &'static str)]) {
        let mut request = tonic::Request::new(());
        for (name, value) in metadata {
            request.metadata_mut().insert(*name, value.parse().expect("an ASCII value"));
        }
        let answer = match ulo_grpc::outgoing(exec, request) {
            Ok(sent) => Ok(sent
                .metadata()
                .iter()
                .filter_map(|entry| match entry {
                    tonic::metadata::KeyAndValueRef::Ascii(key, value) => Some((key.as_str().to_owned(), value.to_str().ok()?.to_owned())),
                    tonic::metadata::KeyAndValueRef::Binary(..) => None,
                })
                .collect()),
            Err(status) => {
                use tonic_types::StatusExt as _;
                let reason = status.get_details_error_info().map(|info| info.reason);
                Err((status.code(), reason))
            }
        };
        self.rec.0.grpc.lock().unwrap_or_else(PoisonError::into_inner).push((label, answer));
    }
}

/// The RPC side: each handler records the context its execution carries; a request forwards it.
#[injectable]
struct Handlers {
    rec: Dep<Rec>,
    out: Dep<Out>,
}

#[routes]
impl Handlers {
    #[ulo_rpc::message("in.req")]
    async fn request(&self, exec: ExecutionRef) -> u32 {
        self.rec.saw("rpc", &exec);
        self.rec.emitted("rpc-forward", self.out.0.emit("out.evt", &1u32).within(&exec).await);
        1
    }

    #[ulo_rpc::event("in.evt")]
    async fn event(&self, exec: ExecutionRef, _payload: Payload<u32>) {
        self.rec.saw("rpc-event", &exec);
    }
}

struct Root {
    rec: Rec,
    out: Arc<Mutex<Option<Out>>>,
    source: Option<Arc<dyn TraceSource>>,
    otel: Option<ulo_otel::Otel>,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.rec.clone());
        m.value(self.out.lock().unwrap_or_else(PoisonError::into_inner).take().expect("registered once"));
        m.controller::<Edge>();
        m.controller::<Handlers>();
        if let Some(source) = &self.source {
            m.import(Sourced(Arc::clone(source)));
        }
        if let Some(otel) = &self.otel {
            m.import(otel.module());
        }
    }
}

/// A backend that serves nothing itself: the test calls `AppService` as a backend would.
#[derive(Clone, Default)]
struct Keeper {
    svc: Arc<OnceLock<AppService>>,
    drained: Arc<Notify>,
}

impl Backend for Keeper {
    const NAME: &'static str = "keeper";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(&mut self, _listeners: Vec<BoundListener>, _tls: Option<Arc<ServerConfig>>, svc: AppService, _cfg: &HttpConfig) -> Result<(), BoxError> {
        let _ = self.svc.set(svc);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.drained.notified().await;
        Ok(())
    }

    async fn drain(&self) {
        self.drained.notify_one();
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

#[derive(Default)]
struct Setup {
    /// Binds the `Fixed` source, which originates the context.
    fixed: bool,
    mode: Option<TraceMode>,
    otel: Option<ulo_otel::Otel>,
}

struct Hub {
    rec: Rec,
    wire: Wire,
    deliveries: mpsc::UnboundedSender<Delivery>,
    svc: AppService,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    ids: AtomicUsize,
}

async fn start(setup: Setup) -> Hub {
    let rec = Rec::default();
    let (wire, deliveries) = Wire::new();
    let client = RpcClient::new(wire.clone(), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("the client builds over the wire");
    let keeper = Keeper::default();
    let source: Option<Arc<dyn TraceSource>> = setup.fixed.then(|| Arc::new(Fixed { rec: rec.clone() }) as Arc<dyn TraceSource>);
    let root = Root { rec: rec.clone(), out: Arc::new(Mutex::new(Some(Out(client)))), source, otel: setup.otel };
    let mut builder = App::builder(root).runtime(ulo_runtime_tokio::Tokio::current());
    if let Some(mode) = setup.mode {
        builder = builder.trace(mode);
    }
    let app = within("the start", async {
        builder
            .wire()
            .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
            .connect()
            .await
            .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
            .bind(ulo_http::Server::with_backend("127.0.0.1:0", keeper.clone()))
            .bind(ulo_rpc::Server::new(wire.clone()))
            .listen()
            .await
            .unwrap_or_else(|error| panic!("the app did not listen: {error}"))
    })
    .await;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    let svc = keeper.svc.get().cloned().expect("the backend was handed the service");
    Hub { rec, wire, deliveries, svc, handle, serving, ids: AtomicUsize::new(1) }
}

impl Hub {
    /// A GET with the given headers; the status of the answer.
    async fn get(&self, path: &str, headers: &[(&str, &str)]) -> StatusCode {
        let mut builder = http::Request::builder().uri(path);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let (head, ()) = builder.body(()).expect("the request builds").into_parts();
        let request = Request { head, body: HttpBody::empty(), conn: ConnInfo::new(http::Version::HTTP_11), upgrade: None };
        let response = within("the HTTP call", self.svc.call(request)).await;
        let status = response.status();
        let mut body = response.into_body();
        // Read to the end, as a backend writing it does.
        within("the body", async {
            while let Some(frame) = std::future::poll_fn(|cx| http_body::Body::poll_frame(std::pin::Pin::new(&mut body), cx)).await {
                if frame.is_err() {
                    break;
                }
            }
        })
        .await;
        status
    }

    /// Delivers `frame` and, for a request, waits for the server's last frame.
    async fn deliver(&self, frame: Frame) -> Vec<Frame> {
        let event = matches!(frame, Frame::Evt { .. });
        let (frames, mut written) = mpsc::unbounded_channel();
        let reply = ReplyPath::new(move |frame: Frame| {
            let sent = frames.send(frame).map_err(BoxError::from);
            Box::pin(async move { sent })
        });
        let (settle, mut settled) = mpsc::unbounded_channel::<bool>();
        let ack = Ack::new(move |accepted| {
            let _ = settle.send(accepted);
        });
        self.deliveries.send(Delivery { frame, reply: (!event).then_some(reply), ack }).expect("the server reads its link");
        within("the server to answer and settle", async {
            let mut out = Vec::new();
            if !event {
                while let Some(frame) = written.recv().await {
                    let last = matches!(frame, Frame::Res { .. } | Frame::Err { .. } | Frame::End { .. });
                    out.push(frame);
                    if last {
                        break;
                    }
                }
            }
            let _ = settled.recv().await;
            out
        })
        .await
    }

    async fn request(&self, headers: &[(&str, &str)]) -> Vec<Frame> {
        let id = self.ids.fetch_add(1, Ordering::SeqCst) as u64;
        self.deliver(Frame::Req { id, pattern: "in.req".into(), headers: call_headers(headers), data: Data::new(b"null".as_slice()) }).await
    }

    async fn event(&self, headers: &[(&str, &str)]) {
        self.deliver(Frame::Evt { pattern: "in.evt".into(), headers: call_headers(headers), data: Data::new(b"7".as_slice()) }).await;
    }

    async fn stop(self) {
        let Hub { handle, serving, deliveries, .. } = self;
        drop(deliveries);
        within("the close", async {
            let _ = handle.close(Signal::new("propagation test")).await;
            let _ = serving.await;
        })
        .await;
    }
}

fn call_headers(headers: &[(&str, &str)]) -> CallHeaders {
    let mut out = CallHeaders::new();
    for (name, value) in headers {
        out.insert(*name, *value);
    }
    out
}

fn context(rec: &Rec) -> TraceContext {
    rec.last_seen().expect("the handler's execution carried a context")
}

/// HTTP: the request's `traceparent` and `tracestate` become the execution's `Received` context,
/// the fields parsed and the text kept as it arrived.
#[tokio::test(flavor = "current_thread")]
async fn an_http_traceparent_becomes_the_execution_s_received_context() {
    let hub = start(Setup::default()).await;
    assert_eq!(hub.get("/seen", &[("traceparent", TRACEPARENT), ("tracestate", TRACESTATE)]).await, StatusCode::OK);
    let received = context(&hub.rec);
    assert_eq!(received.origin(), TraceOrigin::Received);
    assert_eq!(received.trace_id(), TRACE_ID);
    assert_eq!(received.span_id(), SPAN_ID);
    assert_eq!(received.flags(), 0x01);
    assert!(received.sampled());
    assert_eq!(received.traceparent(), TRACEPARENT);
    assert_eq!(received.tracestate().as_deref(), Some(TRACESTATE));
    assert_eq!(received.state().get("congo"), Some("t61rcWkgMzE"));
    hub.stop().await;
}

/// A header that is not a valid context is ignored and `tracestate` with it: the request succeeds
/// and, with no `TraceSource`, the execution carries none. A valid `traceparent` is never spoiled
/// by a bad `tracestate`.
#[tokio::test(flavor = "current_thread")]
async fn an_invalid_context_is_ignored_and_the_request_still_succeeds() {
    let hub = start(Setup::default()).await;
    let invalid: [(&str, &[(&str, &str)]); 6] = [
        ("version ff", &[("traceparent", "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01")]),
        ("an all-zero trace id", &[("traceparent", "00-00000000000000000000000000000000-00f067aa0ba902b7-01")]),
        ("an all-zero span id", &[("traceparent", "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01")]),
        ("uppercase hex", &[("traceparent", "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01")]),
        ("two traceparent fields", &[("traceparent", TRACEPARENT), ("traceparent", TRACEPARENT)]),
        ("a tracestate with no traceparent", &[("tracestate", TRACESTATE)]),
    ];
    for (what, headers) in invalid {
        assert_eq!(hub.get("/seen", headers).await, StatusCode::OK, "{what}");
        assert!(hub.rec.last_seen().is_none(), "{what}: the execution carries no context");
    }

    assert_eq!(hub.get("/seen", &[("traceparent", TRACEPARENT), ("tracestate", "no-equals-sign")]).await, StatusCode::OK);
    let received = context(&hub.rec);
    assert_eq!(received.traceparent(), TRACEPARENT, "a bad tracestate never affects traceparent");
    assert!(received.state().is_empty(), "the list that failed the grammar is not parsed into members");
    hub.stop().await;
}

/// A call made `.within` the execution forwards both headers exactly as received, a version this
/// implementation does not know and Level 2's `random` flag included.
#[tokio::test(flavor = "current_thread")]
async fn a_received_context_is_forwarded_unchanged_on_an_outgoing_rpc_call() {
    let hub = start(Setup::default()).await;
    for (traceparent, tracestate) in [(TRACEPARENT, TRACESTATE), (FUTURE, "a=1, b=2"), (FUTURE, "")] {
        let mut headers = vec![("traceparent", traceparent)];
        if !tracestate.is_empty() {
            headers.push(("tracestate", tracestate));
        }
        let before = hub.wire.sent().len();
        assert_eq!(hub.get("/forward", &headers).await, StatusCode::OK);
        assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("forward", None)), "the event was sent");
        let sent = hub.wire.headers_of(before);
        assert_eq!(sent.get("traceparent"), Some(traceparent), "traceparent as received");
        let expected_state = if tracestate.is_empty() { None } else { Some(tracestate) };
        assert_eq!(sent.get("tracestate"), expected_state, "tracestate as received, the whitespace inside it included");
        assert_eq!(hub.wire.sent().len(), before + 1, "one frame for one call");
    }
    hub.stop().await;
}

/// A call made with no `.within` carries no context, and its headers are the caller's own: the
/// client has no mode of its own.
#[tokio::test(flavor = "current_thread")]
async fn a_call_outside_any_execution_context_keeps_the_callers_trace_headers() {
    let hub = start(Setup::default()).await;
    // The execution carries a context, and the call is made with no `.within`.
    assert_eq!(hub.get("/forward-plain", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
    assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("plain", None)), "both headers are ordinary without `.within`");
    let sent = hub.wire.headers_of(0);
    assert_eq!(sent.get("traceparent"), Some(CALLER_TRACEPARENT));
    assert_eq!(sent.get("tracestate"), Some("mine=1"));
    hub.stop().await;
}

/// An RPC request or event carrying `traceparent` in its headers is read as HTTP's is, and a
/// call the handler makes `.within` forwards the same bytes; headers off the grammar leave the
/// handler with no context.
#[tokio::test(flavor = "current_thread")]
async fn an_rpc_traceparent_becomes_the_received_context_and_is_forwarded_unchanged() {
    let hub = start(Setup::default()).await;
    let frames = hub.request(&[("traceparent", FUTURE), ("tracestate", TRACESTATE)]).await;
    assert!(matches!(frames.as_slice(), [Frame::Res { .. }]), "the request is answered: {frames:?}");
    let received = context(&hub.rec);
    assert_eq!(received.origin(), TraceOrigin::Received);
    assert_eq!((received.trace_id(), received.span_id(), received.flags()), (TRACE_ID, SPAN_ID, 0x03));
    assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("rpc-forward", None)));
    let sent = hub.wire.headers_of(0);
    assert_eq!(sent.get("traceparent"), Some(FUTURE));
    assert_eq!(sent.get("tracestate"), Some(TRACESTATE));

    // An event carries the context to its handler too, though it carries no deadline.
    hub.event(&[("traceparent", TRACEPARENT)]).await;
    hub.rec.until_seen(2).await;
    let (label, event) = hub.rec.seen().pop().expect("the event's handler ran");
    assert_eq!(label, "rpc-event");
    assert_eq!(event.expect("the event's context").traceparent(), TRACEPARENT);

    // Header names match without regard to case, as W3C Trace Context has them.
    hub.event(&[("TraceParent", TRACEPARENT)]).await;
    hub.rec.until_seen(3).await;
    assert_eq!(hub.rec.last_seen().expect("a context").trace_id(), TRACE_ID);

    for headers in [vec![("traceparent", "garbage")], vec![("traceparent", TRACEPARENT), ("Traceparent", TRACEPARENT)], vec![("tracestate", TRACESTATE)]] {
        let frames = hub.request(&headers).await;
        assert!(matches!(frames.as_slice(), [Frame::Res { .. }]), "an invalid context never fails the call: {frames:?}");
        assert!(hub.rec.last_seen().is_none(), "{headers:?}: no context");
    }
    hub.stop().await;
}

/// With tracing off nothing is read: the execution carries no context, and `traceparent` is an
/// ordinary header to a client, which forwards what the caller set.
#[tokio::test(flavor = "current_thread")]
async fn with_tracing_off_the_header_is_ordinary_and_the_execution_carries_nothing() {
    let hub = start(Setup { mode: Some(TraceMode::Off), ..Setup::default() }).await;
    assert_eq!(hub.get("/forward-traceparent", &[("traceparent", TRACEPARENT), ("tracestate", TRACESTATE)]).await, StatusCode::OK);
    assert!(hub.rec.last_seen().is_none(), "nothing was read");
    assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("traceparent", None)), "not refused: the client writes none of its own");
    assert_eq!(hub.wire.headers_of(0).get("traceparent"), Some(CALLER_TRACEPARENT), "the caller's header passes untouched");

    hub.request(&[("traceparent", TRACEPARENT)]).await;
    assert!(hub.rec.last_seen().is_none(), "an RPC frame's header is ordinary too");
    hub.stop().await;
}

/// The reserved-header refusals of the RPC client: a caller's `traceparent` within an execution
/// carrying a context, and its `tracestate` when that context was received, are `BadRequest` with
/// the reason `reserved_header`, and nothing is sent.
#[tokio::test(flavor = "current_thread")]
async fn the_rpc_client_refuses_a_caller_s_traceparent_and_a_tracestate_on_a_received_context() {
    let hub = start(Setup::default()).await;
    assert_eq!(hub.get("/forward-traceparent", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
    assert_eq!(hub.get("/forward-tracestate", &[("traceparent", TRACEPARENT), ("tracestate", TRACESTATE)]).await, StatusCode::OK);
    let refused = Some((ErrorKind::BadRequest, Some("reserved_header".to_owned())));
    assert_eq!(hub.rec.emitted_outcomes(), [("traceparent", refused.clone()), ("tracestate", refused)]);
    assert!(hub.wire.sent().is_empty(), "a refused call is never sent");

    // Within an execution that carries no context both are ordinary headers.
    assert_eq!(hub.get("/forward-tracestate", &[]).await, StatusCode::OK);
    assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("tracestate", None)));
    assert_eq!(hub.wire.headers_of(0).get("tracestate"), Some("mine=1"));
    hub.stop().await;
}

/// A context originated here, by a `TraceSource`, is written as version `00` with the sampled flag
/// as its one flag and the execution's own span id: the received flags beyond `sampled` are not
/// passed on. The source is handed the received context.
#[tokio::test(flavor = "current_thread")]
async fn a_context_originated_here_injects_a_fresh_traceparent() {
    let hub = start(Setup { fixed: true, ..Setup::default() }).await;
    assert_eq!(hub.get("/forward", &[("traceparent", FUTURE), ("tracestate", TRACESTATE)]).await, StatusCode::OK);

    let handed = hub.rec.inbound().pop().flatten().expect("the source was handed the inbound context");
    assert_eq!(handed.origin(), TraceOrigin::Received);
    assert_eq!(handed.traceparent(), FUTURE, "the received text, before anything was changed");

    let own = context(&hub.rec);
    assert_eq!(own.origin(), TraceOrigin::Originated, "the handler's execution carries the source's answer");
    assert_eq!(own.span_id(), OWN_SPAN);
    let sent = hub.wire.headers_of(0);
    assert_eq!(sent.get("traceparent"), Some("00-4bf92f3577b34da6a3ce929d0e0e4736-aaaaaaaaaaaaaaaa-01"), "version 00, the execution's span, only the sampled flag");
    assert_eq!(sent.get("tracestate"), Some(TRACESTATE), "the parsed list, written back in order");

    // No inbound context: the source starts the trace.
    assert_eq!(hub.get("/forward", &[]).await, StatusCode::OK);
    assert_eq!(hub.rec.inbound().last(), Some(&None), "nothing was received, so the source was handed none");
    let sent = hub.wire.headers_of(1);
    assert_eq!(sent.get("traceparent"), Some("00-11111111111111111111111111111111-aaaaaaaaaaaaaaaa-01"));
    assert_eq!(sent.get("tracestate"), None);
    hub.stop().await;
}

/// A caller's `tracestate` on an originated context is not refused: its entries enter at the
/// left, and a key already in the list moves to the left with the caller's value. A caller's
/// `traceparent` is still refused.
#[tokio::test(flavor = "current_thread")]
async fn a_caller_s_tracestate_is_merged_into_an_originated_context() {
    let hub = start(Setup { fixed: true, ..Setup::default() }).await;
    assert_eq!(hub.get("/forward-tracestate", &[("traceparent", TRACEPARENT), ("tracestate", "mine=0,vendor1=old")]).await, StatusCode::OK);
    assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("tracestate", None)), "merged, not refused");
    assert_eq!(hub.wire.headers_of(0).get("tracestate"), Some("mine=1,vendor1=old"), "`mine` moved left with the caller's value, the rest in its order");

    assert_eq!(hub.get("/forward-traceparent", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
    assert_eq!(hub.rec.emitted_outcomes().last(), Some(&("traceparent", Some((ErrorKind::BadRequest, Some("reserved_header".to_owned()))))));
    assert_eq!(hub.wire.sent().len(), 1, "only the merged call was sent");
    hub.stop().await;
}

fn grpc_outcome(rec: &Rec, label: &str) -> GrpcAnswer {
    let grpc = rec.0.grpc.lock().unwrap_or_else(PoisonError::into_inner);
    grpc.iter().rev().find(|(name, _)| *name == label).map(|(_, answer)| answer.clone()).unwrap_or_else(|| panic!("`{label}` made no outgoing gRPC call"))
}

fn metadata<'a>(sent: &'a [(String, String)], name: &str) -> Option<&'a str> {
    sent.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

/// `ulo_grpc::outgoing` from an HTTP execution writes the received context into the metadata as it
/// arrived, both headers' text.
#[tokio::test(flavor = "current_thread")]
async fn a_received_context_is_forwarded_unchanged_on_an_outgoing_grpc_call() {
    let hub = start(Setup::default()).await;
    assert_eq!(hub.get("/grpc", &[("traceparent", FUTURE), ("tracestate", TRACESTATE)]).await, StatusCode::OK);
    let sent = grpc_outcome(&hub.rec, "grpc").expect("the request was written");
    assert_eq!(metadata(&sent, "traceparent"), Some(FUTURE));
    assert_eq!(metadata(&sent, "tracestate"), Some(TRACESTATE));

    // No context in the execution: no trace metadata is invented.
    assert_eq!(hub.get("/grpc", &[]).await, StatusCode::OK);
    let sent = grpc_outcome(&hub.rec, "grpc").expect("the request was written");
    assert_eq!(metadata(&sent, "traceparent"), None);
    assert_eq!(metadata(&sent, "tracestate"), None);
    hub.stop().await;
}

/// The reserved-header refusals of `ulo_grpc::outgoing`: `InvalidArgument` carrying an `ErrorInfo`
/// whose reason is `reserved_header`, for a caller's `traceparent` and for a `tracestate` on a
/// received context; an originated context merges the caller's entries instead; with no context
/// both are ordinary metadata.
#[tokio::test(flavor = "current_thread")]
async fn grpc_outgoing_refuses_a_caller_s_traceparent_and_a_tracestate_on_a_received_context() {
    let hub = start(Setup::default()).await;
    let refused = Err((tonic::Code::InvalidArgument, Some("reserved_header".to_owned())));
    assert_eq!(hub.get("/grpc-traceparent", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
    assert_eq!(grpc_outcome(&hub.rec, "grpc-traceparent"), refused);
    assert_eq!(hub.get("/grpc-tracestate", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
    assert_eq!(grpc_outcome(&hub.rec, "grpc-tracestate"), refused);

    assert_eq!(hub.get("/grpc-traceparent", &[]).await, StatusCode::OK);
    let sent = grpc_outcome(&hub.rec, "grpc-traceparent").expect("ordinary metadata with no context");
    assert_eq!(metadata(&sent, "traceparent"), Some(CALLER_TRACEPARENT));
    hub.stop().await;

    let hub = start(Setup { fixed: true, ..Setup::default() }).await;
    assert_eq!(hub.get("/grpc-tracestate", &[("traceparent", TRACEPARENT), ("tracestate", "mine=0,vendor1=old")]).await, StatusCode::OK);
    let sent = grpc_outcome(&hub.rec, "grpc-tracestate").expect("merged into the originated context, not refused");
    assert_eq!(metadata(&sent, "tracestate"), Some("mine=1,vendor1=old"));
    assert_eq!(metadata(&sent, "traceparent"), Some("00-4bf92f3577b34da6a3ce929d0e0e4736-aaaaaaaaaaaaaaaa-01"));
    assert_eq!(hub.get("/grpc-traceparent", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
    assert_eq!(grpc_outcome(&hub.rec, "grpc-traceparent"), refused, "a caller's `traceparent` is refused on an originated context as well");
    hub.stop().await;
}

/// A gRPC server: the context a call's `traceparent` metadata carries is handed to the app's
/// `TraceSource` as the inbound one, `Received`, with the text as it arrived; a call carrying
/// none, or an invalid one, is handed `None`; under `TraceMode::Off` nothing is read and the
/// source is never called.
mod grpc_server {
    use super::*;
    use tonic::transport::Endpoint;
    use tonic_health::pb::HealthCheckRequest;
    use tonic_health::pb::health_client::HealthClient;

    struct GrpcRoot {
        rec: Rec,
    }

    impl Module for GrpcRoot {
        fn identity(&self) -> ModuleIdentity {
            ModuleIdentity::of_type::<Self>()
        }

        fn register(&self, m: &mut ModuleDef<'_>) {
            m.import(Sourced(Arc::new(Fixed { rec: self.rec.clone() })));
        }
    }

    async fn serve(mode: TraceMode) -> (Rec, HealthClient<tonic::transport::Channel>, AppHandle, tokio::task::JoinHandle<()>) {
        let rec = Rec::default();
        let app = within("the start", async {
            App::builder(GrpcRoot { rec: rec.clone() })
                .runtime(ulo_runtime_tokio::Tokio::current())
                .trace(mode)
                .wire()
                .unwrap_or_else(|error| panic!("the gRPC app did not wire: {error}"))
                .connect()
                .await
                .unwrap_or_else(|error| panic!("the gRPC app did not connect: {error}"))
                .bind(ulo_grpc::Server::new("127.0.0.1:0"))
                .listen()
                .await
                .unwrap_or_else(|error| panic!("the gRPC app did not listen: {error}"))
        })
        .await;
        let addr = app.addresses()[0].addr;
        let handle = app.handle();
        let serving = tokio::spawn(async move {
            let _ = app.serve(std::future::pending::<Signal>()).await;
        });
        let channel = within("the channel", async {
            Endpoint::from_shared(format!("http://{addr}")).expect("an endpoint").connect().await.expect("the client connects")
        })
        .await;
        (rec, HealthClient::new(channel), handle, serving)
    }

    async fn check(client: &mut HealthClient<tonic::transport::Channel>, metadata: &[(&'static str, &str)]) {
        let mut request = tonic::Request::new(HealthCheckRequest { service: String::new() });
        for (name, value) in metadata {
            request.metadata_mut().insert(*name, value.parse().expect("an ASCII value"));
        }
        within("the health call", client.check(request)).await.expect("the health check is answered");
    }

    async fn stop(handle: AppHandle, serving: tokio::task::JoinHandle<()>) {
        within("the close", async {
            let _ = handle.close(Signal::new("propagation test")).await;
            let _ = serving.await;
        })
        .await;
    }

    #[tokio::test]
    async fn a_grpc_traceparent_reaches_the_trace_source_as_the_received_context() {
        let (rec, mut client, handle, serving) = serve(TraceMode::On).await;
        check(&mut client, &[("traceparent", FUTURE), ("tracestate", TRACESTATE)]).await;
        check(&mut client, &[]).await;
        check(&mut client, &[("traceparent", "00-00000000000000000000000000000000-00f067aa0ba902b7-01")]).await;
        check(&mut client, &[("tracestate", TRACESTATE)]).await;

        let inbound = rec.inbound();
        assert_eq!(inbound.len(), 4, "the source is called once for every call served");
        let first = inbound[0].as_ref().expect("the first call carried a context");
        assert_eq!(first.origin(), TraceOrigin::Received);
        assert_eq!((first.trace_id(), first.span_id(), first.flags()), (TRACE_ID, SPAN_ID, 0x03));
        assert_eq!(first.traceparent(), FUTURE);
        assert_eq!(first.tracestate().as_deref(), Some(TRACESTATE));
        assert!(inbound[1..].iter().all(Option::is_none), "no header, an all-zero trace id and a lone tracestate are each none: {inbound:?}");
        stop(handle, serving).await;
    }

    #[tokio::test]
    async fn with_tracing_off_nothing_is_read_from_the_metadata() {
        let (rec, mut client, handle, serving) = serve(TraceMode::Off).await;
        check(&mut client, &[("traceparent", TRACEPARENT)]).await;
        assert!(rec.inbound().is_empty(), "no source is called under `TraceMode::Off`");
        stop(handle, serving).await;
    }
}

/// The real `TraceSource`, `ulo-otel`'s, over the same transports: it continues the inbound trace
/// as a child span, answers the execution's own IDs, and parent-based sampling decides what is
/// exported. The subscriber is installed on the test's own thread, which runs the whole app.
mod real {
    use super::*;
    use opentelemetry::trace::{SpanId, SpanKind, TraceId};
    use opentelemetry_sdk::error::OTelSdkResult;
    use opentelemetry_sdk::trace::{SpanData, SpanExporter};
    use tracing_subscriber::layer::SubscriberExt as _;
    use ulo_otel::{Exporter, Otel, OtelOptions};

    #[derive(Clone, Default)]
    struct Collected(Arc<Mutex<Vec<SpanData>>>);

    #[derive(Debug)]
    struct Sink(Arc<Mutex<Vec<SpanData>>>);

    impl SpanExporter for Sink {
        fn export(&self, batch: Vec<SpanData>) -> impl Future<Output = OTelSdkResult> + Send {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).extend(batch);
            async { Ok(()) }
        }
    }

    impl Exporter for Collected {
        type Span = Sink;

        fn build(self, _options: &OtelOptions) -> Result<Sink, BoxError> {
            Ok(Sink(self.0))
        }
    }

    impl Collected {
        fn spans(&self) -> Vec<SpanData> {
            self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
        }
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    /// The app over `options`, the subscriber with its layer installed for the test's thread.
    async fn start_otel(options: OtelOptions) -> (Hub, Collected, tracing::subscriber::DefaultGuard) {
        let spans = Collected::default();
        let otel = Otel::new(options, spans.clone()).expect("the exporter builds");
        let guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(otel.layer()));
        let hub = start(Setup { otel: Some(otel), ..Setup::default() }).await;
        (hub, spans, guard)
    }

    /// A sampled remote parent: the execution continues its trace under a span of its own, the
    /// outgoing call writes that span, and the exported server span is the parent's child.
    #[tokio::test(flavor = "current_thread")]
    async fn the_inbound_trace_is_continued_under_a_span_of_the_execution_s_own() {
        let (hub, spans, _guard) = start_otel(OtelOptions::default()).await;
        assert_eq!(hub.get("/forward", &[("traceparent", TRACEPARENT), ("tracestate", TRACESTATE)]).await, StatusCode::OK);
        let own = context(&hub.rec);
        assert_eq!(own.origin(), TraceOrigin::Originated);
        assert_eq!(own.trace_id(), TRACE_ID, "the same trace");
        assert_ne!(own.span_id(), SPAN_ID, "under a span of its own");
        assert_ne!(own.span_id(), [0; 8]);
        assert!(own.sampled());

        let sent = hub.wire.headers_of(0);
        let expected = format!("00-{}-{}-01", hex(&TRACE_ID), hex(&own.span_id()));
        assert_eq!(sent.get("traceparent"), Some(expected.as_str()), "version 00, the execution's span, the sampled flag");
        assert_eq!(sent.get("tracestate"), Some(TRACESTATE), "the parent's list, carried on");
        hub.stop().await;

        let exported = spans.spans();
        assert_eq!(exported.len(), 1, "one span for one call served: {exported:?}");
        let server = &exported[0];
        assert_eq!(server.span_kind, SpanKind::Server);
        assert_eq!(server.span_context.trace_id(), TraceId::from_bytes(TRACE_ID));
        assert_eq!(server.span_context.span_id().to_bytes(), own.span_id(), "the exported span is the one the execution carried");
        assert_eq!(server.parent_span_id, SpanId::from_bytes(SPAN_ID));
        assert!(server.parent_span_is_remote);
    }

    /// An event the execution emits `.within` itself reaches its handler carrying the context, and
    /// the handler's span is a child of the emitter's: the same trace, the emitter's span as its
    /// parent. The recorded frame is delivered to the server side by hand.
    #[tokio::test(flavor = "current_thread")]
    async fn an_event_s_handler_span_is_a_child_of_the_emitter_s_span() {
        let (hub, spans, _guard) = start_otel(OtelOptions::default()).await;
        assert_eq!(hub.get("/forward", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
        let emitter = context(&hub.rec);

        let Some(Frame::Evt { headers, data, .. }) = hub.wire.sent().into_iter().next() else { panic!("the handler emitted no event") };
        hub.deliver(Frame::Evt { pattern: "in.evt".into(), headers, data }).await;
        hub.rec.until_seen(2).await;
        let handler = hub.rec.last_seen().expect("the event's handler carried a context");
        assert_eq!(handler.trace_id(), TRACE_ID, "the same trace");
        assert_ne!(handler.span_id(), emitter.span_id(), "a span of its own");
        hub.stop().await;

        let exported = spans.spans();
        let consumer = exported.iter().find(|span| span.span_context.span_id().to_bytes() == handler.span_id()).expect("the handler's span was exported");
        assert_eq!(consumer.parent_span_id, SpanId::from_bytes(emitter.span_id()), "the emitter's span is the parent");
        assert!(consumer.parent_span_is_remote, "it arrived in a frame");
        assert_eq!(consumer.span_context.trace_id(), TraceId::from_bytes(TRACE_ID));
    }

    /// A remote parent whose sampled flag is clear exports nothing, and the flag travels on as
    /// zero.
    #[tokio::test(flavor = "current_thread")]
    async fn an_unsampled_remote_parent_exports_nothing_and_forwards_the_flag_clear() {
        let (hub, spans, _guard) = start_otel(OtelOptions::default()).await;
        let unsampled = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
        assert_eq!(hub.get("/forward", &[("traceparent", unsampled)]).await, StatusCode::OK);
        let own = context(&hub.rec);
        assert!(!own.sampled());
        assert_eq!(own.trace_id(), TRACE_ID);
        let expected = format!("00-{}-{}-00", hex(&TRACE_ID), hex(&own.span_id()));
        assert_eq!(hub.wire.headers_of(0).get("traceparent"), Some(expected.as_str()));
        hub.stop().await;
        assert!(spans.spans().is_empty(), "nothing is exported for an unsampled trace: {:?}", spans.spans());
    }

    /// With no inbound context the execution starts a trace: a root span with a trace id of its
    /// own, sampled, and exported with no parent.
    #[tokio::test(flavor = "current_thread")]
    async fn a_call_with_no_inbound_context_starts_a_new_sampled_trace() {
        let (hub, spans, _guard) = start_otel(OtelOptions::default()).await;
        assert_eq!(hub.get("/forward", &[]).await, StatusCode::OK);
        let own = context(&hub.rec);
        assert_eq!(own.origin(), TraceOrigin::Originated);
        assert_ne!(own.trace_id(), [0; 16]);
        assert_ne!(own.trace_id(), TRACE_ID);
        assert!(own.sampled());
        let expected = format!("00-{}-{}-01", hex(&own.trace_id()), hex(&own.span_id()));
        assert_eq!(hub.wire.headers_of(0).get("traceparent"), Some(expected.as_str()));
        hub.stop().await;

        let exported = spans.spans();
        assert_eq!(exported.len(), 1);
        assert_eq!(exported[0].parent_span_id, SpanId::INVALID, "a root span");
        assert_eq!(exported[0].span_context.trace_id().to_bytes(), own.trace_id());
    }

    /// Sampling is parent-based: a ratio of zero samples no root span, and a call whose remote
    /// parent was sampled is exported all the same.
    #[tokio::test(flavor = "current_thread")]
    async fn sampling_is_parent_based_whatever_the_ratio() {
        let options = OtelOptions { sample_ratio: Some(0.0), ..OtelOptions::default() };
        let (hub, spans, _guard) = start_otel(options).await;
        assert_eq!(hub.get("/forward", &[]).await, StatusCode::OK);
        assert!(!context(&hub.rec).sampled(), "a root span the ratio refuses");
        assert!(hub.wire.headers_of(0).get("traceparent").is_some_and(|value| value.ends_with("-00")));

        assert_eq!(hub.get("/forward", &[("traceparent", TRACEPARENT)]).await, StatusCode::OK);
        assert!(context(&hub.rec).sampled(), "the parent's decision stands");
        hub.stop().await;

        let exported = spans.spans();
        assert_eq!(exported.len(), 1, "only the call with a sampled parent: {exported:?}");
        assert_eq!(exported[0].span_context.trace_id(), TraceId::from_bytes(TRACE_ID));
    }
}
