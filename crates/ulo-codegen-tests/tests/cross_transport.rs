//! One app serving HTTP, WebSocket, RPC over TCP and gRPC on loopback port 0, each transport with
//! a call held in its handler, closed by one `close`: the app turns to draining while the four
//! calls are in flight, every one completes once released, the module's hooks run once each, `serve`
//! answers `Ok` with nothing abandoned, and the four addresses stop accepting. Capability: one
//! application across transports, one `close` draining all (G21). The drain's end is rendered per
//! transport, a handler's own answer to it is kept, a spent shutdown cap still writes each
//! rendering, and the servers' closes run at once.

mod support;
mod wire;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;
use ulo::{
    App, CancelReason, Dep, ExecutionRef, FailureReason, HookKind, Key, Limit, Module, ModuleDef, ModuleIdentity, ShutdownFailure, Signal, Transport, TypeName,
    injectable, routes,
};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{self, AddRequest, Number, Sum};
use ulo_grpc::Message as Grpc;
use ulo_rpc::{Payload, RpcClient};
use ulo_transport::CallError;
use ulo_rpc_tcp::Tcp;

use support::{Record, within};

/// Held calls wait here; the test adds a permit per call to let them go.
#[derive(Clone)]
struct Gate {
    semaphore: Arc<Semaphore>,
    entered: Record<&'static str>,
    /// Each held call's transport and cancel reason, written as its handler's future drops.
    dropped: Record<(&'static str, Option<CancelReason>)>,
}

struct OnDrop(&'static str, ExecutionRef, Record<(&'static str, Option<CancelReason>)>);

impl Drop for OnDrop {
    fn drop(&mut self) {
        self.2.push((self.0, self.1.cancel_reason()));
    }
}

impl Gate {
    async fn hold(&self, transport: &'static str, exec: ExecutionRef) {
        let _dropped = OnDrop(transport, exec, self.dropped.clone());
        self.entered.push(transport);
        let permit = self.semaphore.acquire().await.expect("the gate is never closed");
        permit.forget();
    }
}

#[injectable]
struct Web {
    gate: Dep<Gate>,
}

#[routes]
impl Web {
    #[ulo_http::get("/slow")]
    async fn slow(&self, exec: ExecutionRef) -> String {
        self.gate.hold("http", exec).await;
        "http done".to_owned()
    }

    /// Waits for its execution's cancellation, then answers with the reason it read.
    #[ulo_http::get("/observe")]
    async fn observe(&self, exec: ExecutionRef) -> String {
        self.gate.entered.push("http");
        exec.cancelled().await;
        format!("observed {:?}", exec.cancel_reason())
    }
}

#[injectable]
struct Chat {
    gate: Dep<Gate>,
}

#[routes]
#[ulo_ws::gateway(path = "/ws", port = own)]
impl Chat {
    #[ulo_ws::message("slow")]
    async fn slow(&self, exec: ExecutionRef) -> String {
        self.gate.hold("ws", exec).await;
        "ws done".to_owned()
    }

    /// Waits for its execution's cancellation, then answers with the reason it read.
    #[ulo_ws::message("observe")]
    async fn observe(&self, exec: ExecutionRef) -> String {
        self.gate.entered.push("ws");
        exec.cancelled().await;
        format!("observed {:?}", exec.cancel_reason())
    }

    /// Cancels its own execution, yields once, and answers.
    #[ulo_ws::message("self_cancel")]
    async fn self_cancel(&self, exec: ExecutionRef) -> String {
        exec.cancel();
        tokio::task::yield_now().await;
        "answered after its own cancel".to_owned()
    }
}

/// `Counter.Pause` as the two `Chat` handlers above: 1 cancels its own execution, yields and
/// answers; any other value waits for the cancellation and answers. Either answer is the value.
#[injectable]
struct Observer {
    gate: Dep<Gate>,
}

#[routes]
impl Observer {
    #[ulo_grpc::method(pb::counter::Pause)]
    async fn pause(&self, req: Grpc<Number>, exec: ExecutionRef) -> Result<Number, CallError> {
        if req.0.value == 1 {
            exec.cancel();
            tokio::task::yield_now().await;
        } else {
            self.gate.entered.push("grpc");
            exec.cancelled().await;
        }
        Ok(Number { value: req.0.value })
    }
}

#[injectable]
struct Bus {
    gate: Dep<Gate>,
}

#[routes]
impl Bus {
    #[ulo_rpc::message("cross.slow")]
    async fn slow(&self, n: Payload<u32>, exec: ExecutionRef) -> u32 {
        self.gate.hold("rpc", exec).await;
        n.0 + 1
    }

    /// Waits for its execution's cancellation, then answers with the reason it read.
    #[ulo_rpc::message("cross.observe")]
    async fn observe(&self, exec: ExecutionRef) -> String {
        self.gate.entered.push("rpc");
        exec.cancelled().await;
        format!("observed {:?}", exec.cancel_reason())
    }

    /// Cancels its own execution and never answers.
    #[ulo_rpc::message("cross.cancel")]
    async fn cancel(&self, exec: ExecutionRef) -> u32 {
        exec.cancel();
        std::future::pending().await
    }
}

/// How often each lifecycle hook of the root module ran.
#[derive(Default)]
struct Hooks {
    init: AtomicUsize,
    before_shutdown: AtomicUsize,
    destroy: AtomicUsize,
    shutdown: AtomicUsize,
    /// The signals the before-shutdown and shutdown hooks received, in the order they ran.
    signals: std::sync::Mutex<Vec<Signal>>,
}

struct Root {
    gate: Gate,
    grpc_hold: wire::Hold,
    hooks: Arc<Hooks>,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.gate.clone());
        m.value(self.grpc_hold.clone());
        m.value(HooksHandle(Arc::clone(&self.hooks)));
        m.controller::<Web>();
        m.controller::<Chat>();
        m.controller::<Bus>();
        m.controller::<wire::Service>();
        m.controller::<Observer>();
        m.on_init(|hooks: Dep<HooksHandle>| async move {
            hooks.0.init.fetch_add(1, Ordering::SeqCst);
            Ok::<(), Infallible>(())
        });
        m.before_shutdown(|signal: Signal, hooks: Dep<HooksHandle>| async move {
            hooks.0.before_shutdown.fetch_add(1, Ordering::SeqCst);
            hooks.0.signals.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(signal);
        });
        m.on_shutdown(|signal: Signal, hooks: Dep<HooksHandle>| async move {
            hooks.0.shutdown.fetch_add(1, Ordering::SeqCst);
            hooks.0.signals.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(signal);
        });
        m.on_destroy(|hooks: Dep<HooksHandle>| async move {
            hooks.0.destroy.fetch_add(1, Ordering::SeqCst);
        });
    }
}

struct HooksHandle(Arc<Hooks>);

fn address(addresses: &[ulo::BoundAddr], transport: &str) -> SocketAddr {
    let found: Vec<_> = addresses.iter().filter(|bound| bound.transport == transport).collect();
    assert_eq!(found.len(), 1, "expected one `{transport}` address among {addresses:?}");
    found[0].addr
}

async fn http_slow(addr: SocketAddr) -> (String, String) {
    http_call(addr, "/slow").await
}

/// `GET path` on its own connection: the status line and the body, read until the server closes.
async fn http_call(addr: SocketAddr, path: &str) -> (String, String) {
    let mut stream = TcpStream::connect(addr).await.expect("the HTTP server accepts");
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: cross\r\nConnection: close\r\n\r\n").as_bytes()).await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read to its end");
    let text = String::from_utf8_lossy(&answer).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (head.lines().next().unwrap_or("").to_owned(), body.to_owned())
}

/// How a held WebSocket call ended.
#[derive(Debug)]
enum WsEnd {
    /// A close frame: its code and reason.
    Closed(Option<(u16, String)>),
    /// The socket ended without a close frame; the reason is read through `Debug` in a failure.
    #[allow(dead_code)]
    Cut(String),
    /// A text frame answered the call.
    Answered(Value),
}

async fn ws_end(addr: SocketAddr) -> WsEnd {
    ws_call(addr, "slow").await
}

async fn ws_call(addr: SocketAddr, event: &str) -> WsEnd {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("the WebSocket upgrade");
    socket.send(Message::text(json!({ "event": event, "id": 1 }).to_string())).await.expect("the message is sent");
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return WsEnd::Answered(serde_json::from_str(text.as_str()).expect("a JSON frame")),
            Some(Ok(Message::Close(frame))) => return WsEnd::Closed(frame.map(|frame| (u16::from(frame.code), frame.reason.to_string()))),
            Some(Ok(_)) => continue,
            Some(Err(error)) => return WsEnd::Cut(error.to_string()),
            None => return WsEnd::Cut("the stream ended".to_owned()),
        }
    }
}

async fn pause(addr: SocketAddr, value: i64) -> Result<tonic::Response<Number>, tonic::Status> {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}")).expect("an endpoint").connect().await.expect("the gRPC channel");
    CounterClient::new(channel).pause(Number { value }).await
}

async fn refuses(addr: SocketAddr) -> bool {
    for _ in 0..100 {
        if TcpStream::connect(addr).await.is_err() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    false
}

/// The app listening on all four transports, and what a test reads back from it.
struct Running {
    gate: Gate,
    grpc_hold: wire::Hold,
    hooks: Arc<Hooks>,
    handle: ulo::AppHandle,
    serving: tokio::task::JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>>,
    http: SocketAddr,
    ws: SocketAddr,
    rpc: SocketAddr,
    grpc: SocketAddr,
}

/// The four calls, each started and held in its handler.
struct Held {
    http: tokio::task::JoinHandle<(String, String)>,
    ws: tokio::task::JoinHandle<WsEnd>,
    rpc: tokio::task::JoinHandle<Result<u32, ulo_rpc::RpcError>>,
    grpc: tokio::task::JoinHandle<Result<tonic::Response<Sum>, tonic::Status>>,
}

impl Held {
    fn any_finished(&self) -> bool {
        self.http.is_finished() || self.ws.is_finished() || self.rpc.is_finished() || self.grpc.is_finished()
    }
}

async fn launch(drain: Duration) -> Running {
    launch_capped(drain, None).await
}

/// [`launch`] with `shutdown_timeout` set to `cap`.
async fn launch_capped(drain: Duration, cap: Option<Duration>) -> Running {
    let gate = Gate { semaphore: Arc::new(Semaphore::new(0)), entered: Record::new(), dropped: Record::new() };
    let grpc_hold = wire::Hold::new();
    let hooks = Arc::new(Hooks::default());
    let mut builder = App::builder(Root { gate: gate.clone(), grpc_hold: grpc_hold.clone(), hooks: Arc::clone(&hooks) })
        .runtime(ulo_runtime_tokio::Tokio::current())
        .drain_timeout(drain);
    if let Some(cap) = cap {
        builder = builder.shutdown_timeout(cap);
    }
    let app = builder
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));

    let addresses = app.addresses();
    let mut keys: Vec<&str> = addresses.iter().map(|bound| bound.transport).collect();
    keys.sort_unstable();
    let mut expected = vec![ulo_http::Http::KEY, ulo_ws::Ws::KEY, ulo_rpc::Rpc::KEY, ulo_grpc::Grpc::KEY];
    expected.sort_unstable();
    assert_eq!(keys, expected, "one address per transport, each on its own port");
    let ports: std::collections::HashSet<u16> = addresses.iter().map(|bound| bound.addr.port()).collect();
    assert_eq!(ports.len(), 4, "four distinct ports in {addresses:?}");
    assert!(addresses.iter().all(|bound| !bound.tls), "a plaintext server marked secure in {addresses:?}");
    assert_eq!(hooks.init.load(Ordering::SeqCst), 1, "the init hook ran once, before listen");

    let http = address(&addresses, ulo_http::Http::KEY);
    let ws = address(&addresses, ulo_ws::Ws::KEY);
    let rpc = address(&addresses, ulo_rpc::Rpc::KEY);
    let grpc = address(&addresses, ulo_grpc::Grpc::KEY);
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });
    Running { gate, grpc_hold, hooks, handle, serving, http, ws, rpc, grpc }
}

impl Running {
    /// Starts one call on each transport and waits until every handler holds it.
    async fn hold_four(&self) -> Held {
        let (http, ws, rpc, grpc) = (self.http, self.ws, self.rpc, self.grpc);
        let held = Held {
            http: tokio::spawn(http_slow(http)),
            ws: tokio::spawn(ws_end(ws)),
            rpc: tokio::spawn(async move {
                let client = RpcClient::new(Tcp::new(rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");
                client.request::<u32, u32>("cross.slow", &41).timeout(Duration::from_secs(20)).await
            }),
            grpc: tokio::spawn(async move {
                let channel = tonic::transport::Endpoint::from_shared(format!("http://{grpc}")).expect("an endpoint").connect().await.expect("the gRPC channel");
                CounterClient::new(channel).add(AddRequest { a: 100, b: 1 }).await
            }),
        };
        self.gate.entered.at_least(3, "the HTTP, WebSocket and RPC handlers entering").await;
        self.grpc_hold.entered.at_least(1, "the gRPC handler entering").await;
        let mut entered = self.gate.entered.snapshot();
        entered.sort_unstable();
        assert_eq!(entered, vec!["http", "rpc", "ws"], "each held call reached the handler of its own transport");
        held
    }

    /// One RPC call held in its handler.
    async fn hold_rpc(&self) -> tokio::task::JoinHandle<Result<u32, ulo_rpc::RpcError>> {
        let rpc = self.rpc;
        let call = tokio::spawn(async move {
            let client = RpcClient::new(Tcp::new(rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");
            client.request::<u32, u32>("cross.slow", &41).timeout(Duration::from_secs(20)).await
        });
        self.gate.entered.at_least(1, "the RPC handler entering").await;
        assert_eq!(self.gate.entered.snapshot(), vec!["rpc"], "the held call reached the RPC handler");
        call
    }

    /// One gRPC call held in its handler.
    async fn hold_grpc(&self) -> tokio::task::JoinHandle<Result<tonic::Response<Sum>, tonic::Status>> {
        let grpc = self.grpc;
        let call = tokio::spawn(async move {
            let channel = tonic::transport::Endpoint::from_shared(format!("http://{grpc}")).expect("an endpoint").connect().await.expect("the gRPC channel");
            CounterClient::new(channel).add(AddRequest { a: 100, b: 1 }).await
        });
        self.grpc_hold.entered.at_least(1, "the gRPC handler entering").await;
        assert!(self.gate.entered.snapshot().is_empty(), "a call other than the gRPC one entered a handler");
        call
    }

    /// One WebSocket call held in its handler.
    async fn hold_ws(&self) -> tokio::task::JoinHandle<WsEnd> {
        let call = tokio::spawn(ws_end(self.ws));
        self.gate.entered.at_least(1, "the WebSocket handler entering").await;
        assert_eq!(self.gate.entered.snapshot(), vec!["ws"], "the held call reached the WebSocket handler");
        call
    }

    /// Closes with a drain window that expires, returning the number of executions abandoned.
    async fn close_abandoning(&self) -> usize {
        let closed = within("close", self.handle.close(Signal::new("cross"))).await;
        match &closed {
            Ok(shutdown) => shutdown.abandoned,
            Err(error) => error.report.abandoned,
        }
    }

    /// Starts the close and waits until the app reports draining.
    async fn begin_close(&self) -> tokio::task::JoinHandle<Result<ulo::Shutdown, ulo::ShutdownError>> {
        let handle = self.handle.clone();
        let closing = tokio::spawn({
            let handle = handle.clone();
            async move { handle.close(Signal::new("cross")).await }
        });
        within("the app turning to draining", async {
            while !handle.is_draining() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        closing
    }
}

#[tokio::test]
async fn one_close_drains_all_four_transports_and_every_held_call_completes() {
    let app = launch(Duration::from_secs(5)).await;
    let held = app.hold_four().await;

    let closing = app.begin_close().await;
    assert!(!closing.is_finished(), "close returned while four calls were held");
    assert!(!held.any_finished(), "a held call ended when the drain began");
    assert_eq!(app.hooks.before_shutdown.load(Ordering::SeqCst), 1, "the before-shutdown hook ran at the start of the sequence");
    assert_eq!(app.hooks.destroy.load(Ordering::SeqCst), 0, "destroy hooks wait for the drain");
    assert_eq!(app.hooks.shutdown.load(Ordering::SeqCst), 0, "shutdown hooks wait for the drain");
    // Every transport entered its own drain: none accepts a new connection while its held call runs.
    for (name, addr) in [("http", app.http), ("ws", app.ws), ("rpc", app.rpc), ("grpc", app.grpc)] {
        assert!(refuses(addr).await, "the {name} server was still accepting during the drain");
    }

    app.gate.semaphore.add_permits(3);
    app.grpc_hold.gate.notify_one();

    let (status, body) = within("the HTTP call", held.http).await.expect("the HTTP task");
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(body.trim().rsplit('\n').next().map(str::trim), Some("http done"), "body: {body:?}");
    let reply = within("the WebSocket call", held.ws).await.expect("the WebSocket task");
    match reply {
        WsEnd::Answered(reply) => assert_eq!(reply, json!({ "id": 1, "data": "ws done" })),
        other => panic!("the held WebSocket call was not answered with its result: {other:?}"),
    }
    let reply = within("the RPC call", held.rpc).await.expect("the RPC task");
    assert_eq!(reply.unwrap_or_else(|error| panic!("the RPC call failed: {error}")), 42);
    let reply = within("the gRPC call", held.grpc).await.expect("the gRPC task");
    assert_eq!(reply.unwrap_or_else(|status| panic!("the gRPC call failed: {status:?}")).into_inner(), Sum { sum: 101 });

    let closed = within("close", closing).await.expect("the close task");
    let shutdown = closed.unwrap_or_else(|error| panic!("close failed: {error}"));
    assert_eq!(shutdown.abandoned, 0, "an execution outlived the drain");
    let served = within("serve", app.serving).await.expect("the serve task");
    let served = served.unwrap_or_else(|error| panic!("serve failed: {error}"));
    assert_eq!(served.signal, Signal::new("cross"), "serve answers with the signal that won");
    assert_eq!(app.hooks.before_shutdown.load(Ordering::SeqCst), 1);
    assert_eq!(app.hooks.destroy.load(Ordering::SeqCst), 1, "the destroy hook ran once");
    assert_eq!(app.hooks.shutdown.load(Ordering::SeqCst), 1, "the shutdown hook ran once");
    let signals = app.hooks.signals.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    assert_eq!(signals, vec![Signal::new("cross"), Signal::new("cross")], "the hooks received the closing signal");
    let mut dropped = app.gate.dropped.snapshot();
    dropped.sort_unstable_by_key(|(transport, _)| *transport);
    assert_eq!(dropped, vec![("http", None), ("rpc", None), ("ws", None)], "a completed call was cancelled");
    assert_eq!(app.grpc_hold.dropped.snapshot(), vec![None], "the completed gRPC call was cancelled");
    for (name, addr) in [("http", app.http), ("ws", app.ws), ("rpc", app.rpc), ("grpc", app.grpc)] {
        assert!(refuses(addr).await, "the {name} address still accepts connections after close");
    }
}

#[tokio::test]
async fn calls_still_held_when_the_drain_window_ends_are_abandoned_and_close_finishes_on_all_four() {
    let app = launch(Duration::from_millis(400)).await;
    let held = app.hold_four().await;

    let closed = within("close", app.handle.close(Signal::new("cross"))).await;
    let abandoned = match &closed {
        Ok(shutdown) => shutdown.abandoned,
        Err(error) => error.report.abandoned,
    };
    assert_eq!(abandoned, 4, "one abandoned execution per held call: {closed:?}");
    let _ = within("serve", app.serving).await.expect("the serve task");
    assert_eq!(app.hooks.before_shutdown.load(Ordering::SeqCst), 1);
    assert_eq!(app.hooks.destroy.load(Ordering::SeqCst), 1, "the destroy hook ran once");
    // Each abandoned call's handler is dropped cancelled by the drain, one per transport.
    let mut dropped = app.gate.dropped.at_least(3, "the three held handlers dropping").await;
    dropped.sort_unstable_by_key(|(transport, _)| *transport);
    let drain = Some(CancelReason::Drain);
    assert_eq!(dropped, vec![("http", drain), ("rpc", drain), ("ws", drain)]);
    assert_eq!(app.grpc_hold.dropped.at_least(1, "the held gRPC handler dropping").await, vec![drain]);
    for (name, addr) in [("http", app.http), ("ws", app.ws), ("rpc", app.rpc), ("grpc", app.grpc)] {
        assert!(refuses(addr).await, "the {name} address still accepts connections after close");
    }

    // No held call was answered with its handler's result; each transport renders the
    // abandonment its own way, with the one shutdown phrase.
    let http = within("the HTTP call's end", held.http).await.expect("the HTTP task");
    assert!(!http.1.contains("http done"), "an abandoned HTTP call was answered: {http:?}");
    let rpc = within("the RPC call's end", held.rpc).await.expect("the RPC task");
    let rpc = rpc.expect_err("an abandoned RPC call was answered with its result");
    assert_eq!((rpc.kind(), rpc.message()), (ulo_transport::ErrorKind::Unavailable, SHUTDOWN_PHRASE), "the RPC call: {rpc:?}");
    let grpc = within("the gRPC call's end", held.grpc).await.expect("the gRPC task");
    let grpc = grpc.expect_err("an abandoned gRPC call was answered with its result");
    assert_eq!((grpc.code(), grpc.message()), (tonic::Code::Unavailable, SHUTDOWN_PHRASE), "the gRPC call: {grpc:?}");
    let ws = within("the WebSocket call's end", held.ws).await.expect("the WebSocket task");
    assert!(matches!(&ws, WsEnd::Closed(Some((1001, reason))) if reason == SHUTDOWN_PHRASE), "the WebSocket call: {ws:?}");
}

const SHUTDOWN_PHRASE: &str = "the server is shutting down";

#[tokio::test]
async fn an_rpc_call_abandoned_at_the_drains_end_answers_unavailable_with_the_shutdown_phrase() {
    let app = launch(Duration::from_millis(400)).await;
    let call = app.hold_rpc().await;

    assert_eq!(app.close_abandoning().await, 1, "one abandoned execution for the one held call");
    let error = within("the RPC call's end", call).await.expect("the RPC task").expect_err("an abandoned RPC call was answered with its result");
    assert_eq!(error.kind(), ulo_transport::ErrorKind::Unavailable, "the abandoned call's kind: {error:?}");
    assert_eq!(error.message(), SHUTDOWN_PHRASE, "the abandoned call's phrase");
    assert_eq!(app.gate.dropped.at_least(1, "the held handler dropping").await, vec![("rpc", Some(CancelReason::Drain))], "the handler was cancelled by the drain");
    let _ = within("serve", app.serving).await;
}

#[tokio::test]
async fn a_grpc_call_abandoned_at_the_drains_end_answers_unavailable_with_the_shutdown_phrase() {
    let app = launch(Duration::from_millis(400)).await;
    let call = app.hold_grpc().await;

    assert_eq!(app.close_abandoning().await, 1, "one abandoned execution for the one held call");
    let status = within("the gRPC call's end", call).await.expect("the gRPC task").expect_err("an abandoned gRPC call was answered with its result");
    assert_eq!(status.code(), tonic::Code::Unavailable, "the abandoned call's code: {status:?}");
    assert_eq!(status.message(), SHUTDOWN_PHRASE, "the abandoned call's phrase");
    assert_eq!(app.grpc_hold.dropped.at_least(1, "the held handler dropping").await, vec![Some(CancelReason::Drain)], "the handler was cancelled by the drain");
    let _ = within("serve", app.serving).await;
}

#[tokio::test]
async fn a_websocket_call_abandoned_at_the_drains_end_is_closed_1001_with_the_shutdown_phrase() {
    let app = launch(Duration::from_millis(400)).await;
    let call = app.hold_ws().await;

    assert_eq!(app.close_abandoning().await, 1, "one abandoned execution for the one held call");
    let end = within("the WebSocket call's end", call).await.expect("the WebSocket task");
    match end {
        WsEnd::Closed(Some((code, reason))) => {
            assert_eq!(code, 1001, "the close code of the abandoned call");
            assert_eq!(reason, SHUTDOWN_PHRASE, "the close reason of the abandoned call");
        }
        other => panic!("an abandoned WebSocket call did not end with a close frame: {other:?}"),
    }
    assert_eq!(app.gate.dropped.at_least(1, "the held handler dropping").await, vec![("ws", Some(CancelReason::Drain))], "the handler was cancelled by the drain");
    let _ = within("serve", app.serving).await;
}

/// The handler is polled before the cancellation: one that wakes on the drain's end and answers
/// in that poll is answered as it chose, on all four transports.
#[tokio::test]
async fn a_handler_answering_in_the_poll_the_drains_end_wakes_is_answered_as_it_chose() {
    let app = launch(Duration::from_millis(300)).await;
    let http = tokio::spawn(http_call(app.http, "/observe"));
    let ws = tokio::spawn(ws_call(app.ws, "observe"));
    let rpc = tokio::spawn({
        let rpc = app.rpc;
        async move {
            let client = RpcClient::new(Tcp::new(rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");
            client.request::<u32, String>("cross.observe", &0).timeout(Duration::from_secs(20)).await
        }
    });
    let grpc = tokio::spawn(pause(app.grpc, 2));
    let mut entered = app.gate.entered.at_least(4, "the four handlers waiting on their cancellation").await;
    entered.sort_unstable();
    assert_eq!(entered, vec!["grpc", "http", "rpc", "ws"]);

    assert_eq!(app.close_abandoning().await, 4, "the window ended with the four calls running");
    let observed = "observed Some(Drain)";
    let (status, body) = within("the HTTP call", http).await.expect("the HTTP task");
    assert_eq!((status.as_str(), body.trim().rsplit('\n').next().map(str::trim)), ("HTTP/1.1 200 OK", Some(observed)), "the HTTP handler's own answer: {body:?}");
    match within("the WebSocket call", ws).await.expect("the WebSocket task") {
        WsEnd::Answered(reply) => assert_eq!(reply, json!({ "id": 1, "data": observed })),
        other => panic!("the WebSocket handler's own answer was lost: {other:?}"),
    }
    let reply = within("the RPC call", rpc).await.expect("the RPC task");
    assert_eq!(reply.unwrap_or_else(|error| panic!("the RPC handler's own answer was lost: {error:?}")), observed);
    let reply = within("the gRPC call", grpc).await.expect("the gRPC task");
    assert_eq!(reply.unwrap_or_else(|status| panic!("the gRPC handler's own answer was lost: {status:?}")).into_inner().value, 2);
    let _ = within("serve", app.serving).await;
}

/// Only the drain's cancellation is rendered with the shutdown phrase. A handler that cancels its
/// own execution and answers is answered on WebSocket and gRPC; on RPC one that cancels its own
/// execution and never answers is answered `unavailable` with the cancellation's own phrase.
#[tokio::test]
async fn a_call_its_own_handler_cancels_is_not_answered_as_the_drain() {
    let app = launch(Duration::from_secs(5)).await;
    match within("the WebSocket call", ws_call(app.ws, "self_cancel")).await {
        WsEnd::Answered(reply) => assert_eq!(reply, json!({ "id": 1, "data": "answered after its own cancel" })),
        other => panic!("a WebSocket handler that cancelled itself was not answered: {other:?}"),
    }
    let reply = within("the gRPC call", pause(app.grpc, 1)).await;
    assert_eq!(reply.unwrap_or_else(|status| panic!("a gRPC handler that cancelled itself: {status:?}")).into_inner().value, 1);
    let client = RpcClient::new(Tcp::new(app.rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");
    let error = within("the RPC call", async { client.request::<u32, u32>("cross.cancel", &1).timeout(Duration::from_secs(20)).await })
        .await
        .expect_err("an RPC call its handler cancelled was answered");
    assert_eq!((error.kind(), error.message()), (ulo_transport::ErrorKind::Unavailable, "the call was cancelled"), "{error:?}");
    let _ = within("close", app.handle.close(Signal::new("cross"))).await;
    let _ = within("serve", app.serving).await;
}

/// One controller, one `#[routes]` impl, a handler on HTTP and a handler on RPC; the number of
/// calls it has taken is its own state, so both transports reading one count shows one instance.
#[injectable]
struct Both {
    #[injectable(default)]
    calls: AtomicUsize,
}

#[routes]
impl Both {
    #[ulo_http::get("/both")]
    fn over_http(&self) -> String {
        (self.calls.fetch_add(1, Ordering::SeqCst) + 1).to_string()
    }

    #[ulo_rpc::message("cross.both")]
    fn over_rpc(&self) -> usize {
        self.calls.fetch_add(1, Ordering::SeqCst) + 1
    }
}

struct SharedRoot;

impl Module for SharedRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.controller::<Both>();
    }
}

async fn http_get(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("the HTTP server accepts");
    stream.write_all(format!("GET {path} HTTP/1.1\r\nHost: cross\r\nConnection: close\r\n\r\n").as_bytes()).await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read to its end");
    let text = String::from_utf8_lossy(&answer).into_owned();
    assert!(text.starts_with("HTTP/1.1 200"), "{text}");
    text.rsplit("\r\n\r\n").next().unwrap_or("").trim().rsplit('\n').next().unwrap_or("").trim().to_owned()
}

#[tokio::test]
async fn one_controller_serves_http_and_rpc_from_one_instance() {
    let app = App::builder(SharedRoot)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_rpc::Server::new(Tcp::new("127.0.0.1:0")))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addresses = app.addresses();
    let http = address(&addresses, ulo_http::Http::KEY);
    let rpc = address(&addresses, ulo_rpc::Rpc::KEY);
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });
    let client = RpcClient::new(Tcp::new(rpc), Arc::new(ulo_runtime_tokio::Tokio::current())).expect("a client on the RPC address");

    assert_eq!(within("the first HTTP call", http_get(http, "/both")).await, "1");
    let second: usize = within("the RPC call", async { client.request::<u32, usize>("cross.both", &0).await }).await.unwrap_or_else(|error| panic!("the RPC call failed: {error}"));
    assert_eq!(second, 2, "the RPC handler counted from a controller of its own");
    assert_eq!(within("the second HTTP call", http_get(http, "/both")).await, "3");

    let _ = within("close", handle.close(Signal::new("cross"))).await;
    let _ = within("serve", serving).await;
}

/// A spent shutdown cap gives each server's `close` one poll, and that poll writes the answer a
/// call cancelled at the drain's end needs no wait for: each client reads its transport's
/// rendering of the shutdown, not a reset. Every `close` is then cut by the cap, and the report
/// lists the cuts in bind order between the hooks the cap skipped.
#[tokio::test]
async fn a_spent_cap_still_writes_each_held_calls_shutdown_answer() {
    let app = launch_capped(Duration::from_secs(5), Some(Duration::ZERO)).await;
    let held = app.hold_four().await;

    let error = within("close", app.handle.close(Signal::new("cross"))).await.expect_err("a spent cap records the steps it cut");
    assert_eq!(error.report.abandoned, 4, "one abandoned execution per held call: {error}");
    let mut failures = error.failures.iter();
    for kind in [HookKind::BeforeApplicationShutdown, HookKind::OnModuleDestroy] {
        match failures.next() {
            Some(ShutdownFailure::Hook { hook, key, reason: FailureReason::Skipped }) => {
                assert_eq!(*hook, kind);
                assert_eq!(key.key(), Key::of::<Root, ()>(), "the hook's owner");
            }
            other => panic!("expected the cap to skip the {kind:?} hook, got: {other:?}"),
        }
    }
    let bind_order = [
        TypeName::of::<ulo_http::Http>(),
        TypeName::of::<ulo_ws::Ws>(),
        TypeName::of::<ulo_rpc::Rpc>(),
        TypeName::of::<ulo_grpc::Grpc>(),
    ];
    for expected in bind_order {
        match failures.next() {
            Some(ShutdownFailure::Close { transport, reason: FailureReason::TimedOut { after, limit } }) => {
                assert_eq!(*transport, expected, "the closes the cap cut, in bind order");
                assert_eq!((*after, *limit), (Duration::ZERO, Limit::ShutdownCap), "{transport}'s close");
            }
            other => panic!("expected `{expected}`'s close cut by the cap, got: {other:?}"),
        }
    }
    match failures.next() {
        Some(ShutdownFailure::Hook { hook: HookKind::OnApplicationShutdown, key, reason: FailureReason::Skipped }) => {
            assert_eq!(key.key(), Key::of::<Root, ()>(), "the hook's owner");
        }
        other => panic!("expected the cap to skip the on-shutdown hook, got: {other:?}"),
    }
    assert!(failures.next().is_none(), "a failure past the seven expected: {error}");
    let _ = within("serve", app.serving).await;

    let (status, body) = within("the HTTP call's end", held.http).await.expect("the HTTP task");
    assert_eq!(status, "HTTP/1.1 503 Service Unavailable", "the HTTP call: {body:?}");
    let document: Value = serde_json::from_str(body.trim()).unwrap_or_else(|error| panic!("a problem document, got {body:?}: {error}"));
    assert_eq!(document["detail"], SHUTDOWN_PHRASE, "the HTTP call: {document}");
    let ws = within("the WebSocket call's end", held.ws).await.expect("the WebSocket task");
    assert!(matches!(&ws, WsEnd::Closed(Some((1001, reason))) if reason == SHUTDOWN_PHRASE), "the WebSocket call: {ws:?}");
    let rpc = within("the RPC call's end", held.rpc).await.expect("the RPC task").expect_err("an abandoned RPC call was answered with its result");
    assert_eq!((rpc.kind(), rpc.message()), (ulo_transport::ErrorKind::Unavailable, SHUTDOWN_PHRASE), "the RPC call: {rpc:?}");
    let grpc = within("the gRPC call's end", held.grpc).await.expect("the gRPC task").expect_err("an abandoned gRPC call was answered with its result");
    assert_eq!((grpc.code(), grpc.message()), (tonic::Code::Unavailable, SHUTDOWN_PHRASE), "the gRPC call: {grpc:?}");
}

/// A WebSocket client that sends `slow`, held by the handler, and then reads nothing more, so it
/// never answers the server's close.
async fn frozen_ws(addr: SocketAddr) -> impl Sized {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("the WebSocket upgrade");
    socket.send(Message::text(json!({ "event": "slow", "id": 1 }).to_string())).await.expect("the message is sent");
    socket
}

/// `Counter.Count` as a flood: `up_to` numbers, every one ready, each a ten-byte varint.
#[injectable]
struct Flood;

#[routes]
impl Flood {
    #[ulo_grpc::method(pb::counter::Count)]
    async fn count(&self, req: Grpc<pb::CountRequest>) -> impl futures_util::Stream<Item = Result<Number, CallError>> {
        futures_util::stream::iter((0..i64::from(req.0.up_to)).map(|n| Ok(Number { value: i64::MAX - n })))
    }
}

/// A gRPC client that calls `Count` for more than its 64 KiB HTTP/2 receive windows hold and then
/// reads nothing, so the server's send waits for a `WINDOW_UPDATE` that never comes.
async fn frozen_grpc(addr: SocketAddr) -> impl Sized {
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .expect("an endpoint")
        .initial_stream_window_size(65_535)
        .initial_connection_window_size(65_535)
        .connect()
        .await
        .expect("the gRPC channel");
    let reply = CounterClient::new(channel.clone()).count(pb::CountRequest { up_to: 400_000 }).await.expect("the stream's head");
    (channel, reply.into_inner())
}

/// The `Chat` gateway and the `Count` flood, for an app binding WebSocket and gRPC servers.
struct Frozen(Gate);

impl Module for Frozen {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Chat>();
        m.controller::<Flood>();
    }
}

/// The closes run at once: two WebSocket servers, each with a client that holds a call and then
/// reads nothing, so each server waits out its 500 ms grace for the close handshake its client
/// never completes, and two gRPC servers, each with a client whose reply stream fills its HTTP/2
/// window and is never read, so each waits out its 500 ms grace for a connection that cannot end.
/// `close` takes the drain window and one grace, not four.
#[tokio::test]
async fn four_servers_waiting_out_their_grace_for_frozen_clients_close_within_one_grace() {
    const DRAIN: Duration = Duration::from_millis(200);
    const GRACE: Duration = Duration::from_millis(500);
    let gate = Gate { semaphore: Arc::new(Semaphore::new(0)), entered: Record::new(), dropped: Record::new() };
    let app = App::builder(Frozen(gate.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .drain_timeout(DRAIN)
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addrs = |transport: &str| -> Vec<SocketAddr> { app.addresses().iter().filter(|bound| bound.transport == transport).map(|bound| bound.addr).collect() };
    let (ws, grpc) = (addrs(ulo_ws::Ws::KEY), addrs(ulo_grpc::Grpc::KEY));
    assert_eq!((ws.len(), grpc.len()), (2, 2), "one address per server: {:?}", app.addresses());
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });
    let frozen = (frozen_ws(ws[0]).await, frozen_ws(ws[1]).await, frozen_grpc(grpc[0]).await, frozen_grpc(grpc[1]).await);
    gate.entered.at_least(2, "both WebSocket handlers entering").await;

    let started = std::time::Instant::now();
    let closed = within("close", handle.close(Signal::new("cross"))).await;
    let took = started.elapsed();
    let shutdown = closed.unwrap_or_else(|error| panic!("each close ended within its own bound: {error}"));
    assert_eq!(shutdown.abandoned, 4, "the two held calls and the two frozen streams were abandoned");
    assert!(took >= DRAIN + GRACE, "close returned before a server waited out its grace: {took:?}");
    assert!(took < DRAIN + GRACE + Duration::from_millis(400), "the graces were paid one after the other: {took:?}");
    let _ = within("serve", serving).await;
    let mut dropped = gate.dropped.at_least(2, "the two held handlers dropping").await;
    dropped.sort_unstable_by_key(|(transport, _)| *transport);
    assert_eq!(dropped, vec![("ws", Some(CancelReason::Drain)), ("ws", Some(CancelReason::Drain))]);
    drop(frozen);
}
