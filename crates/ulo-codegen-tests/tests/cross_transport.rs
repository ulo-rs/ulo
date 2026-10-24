//! One app serving HTTP, WebSocket, RPC over TCP and gRPC on loopback port 0, each transport with
//! a call held in its handler, closed by one `close`: the app turns to draining while the four
//! calls are in flight, every one completes once released, the module's hooks run once each, `serve`
//! answers `Ok` with nothing abandoned, and the four addresses stop accepting. Capability: one
//! application across transports, one `close` draining all (G21).

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
use ulo::{App, CancelReason, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, Transport, injectable, routes};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{AddRequest, Sum};
use ulo_rpc::{Payload, RpcClient};
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
    let mut stream = TcpStream::connect(addr).await.expect("the HTTP server accepts");
    stream.write_all(b"GET /slow HTTP/1.1\r\nHost: cross\r\nConnection: close\r\n\r\n").await.expect("the request is written");
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer).await.expect("the answer is read to its end");
    let text = String::from_utf8_lossy(&answer).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    (head.lines().next().unwrap_or("").to_owned(), body.to_owned())
}

async fn ws_slow(addr: SocketAddr) -> Value {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.expect("the WebSocket upgrade");
    socket.send(Message::text(json!({ "event": "slow", "id": 1 }).to_string())).await.expect("the message is sent");
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => return serde_json::from_str(text.as_str()).expect("a JSON frame"),
            Some(Ok(_)) => continue,
            other => panic!("the WebSocket ended without an answer: {other:?}"),
        }
    }
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
    ws: tokio::task::JoinHandle<Value>,
    rpc: tokio::task::JoinHandle<Result<u32, ulo_rpc::RpcError>>,
    grpc: tokio::task::JoinHandle<Result<tonic::Response<Sum>, tonic::Status>>,
}

impl Held {
    fn any_finished(&self) -> bool {
        self.http.is_finished() || self.ws.is_finished() || self.rpc.is_finished() || self.grpc.is_finished()
    }
}

async fn launch(drain: Duration) -> Running {
    let gate = Gate { semaphore: Arc::new(Semaphore::new(0)), entered: Record::new(), dropped: Record::new() };
    let grpc_hold = wire::Hold::new();
    let hooks = Arc::new(Hooks::default());
    let app = App::builder(Root { gate: gate.clone(), grpc_hold: grpc_hold.clone(), hooks: Arc::clone(&hooks) })
        .runtime(ulo_runtime_tokio::Tokio::current())
        .drain_timeout(drain)
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
            ws: tokio::spawn(ws_slow(ws)),
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
    assert_eq!(reply, json!({ "id": 1, "data": "ws done" }));
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

    // No held call was answered with its handler's result.
    let http = within("the HTTP call's end", held.http).await.expect("the HTTP task");
    assert!(!http.1.contains("http done"), "an abandoned HTTP call was answered: {http:?}");
    let rpc = within("the RPC call's end", held.rpc).await.expect("the RPC task");
    assert!(rpc.is_err(), "an abandoned RPC call was answered: {rpc:?}");
    let grpc = within("the gRPC call's end", held.grpc).await.expect("the gRPC task");
    assert!(grpc.is_err(), "an abandoned gRPC call was answered: {grpc:?}");
    held.ws.abort();
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
