//! graphql-transport-ws as the gateway speaks it (transports DESIGN §7; SWITCH_INVENTORY G18):
//! `connection_init` and its acknowledgement, a subscription's `next` and `complete`, an `error`
//! for a request that failed before execution, `ping` and `pong`, the cancellation a client's
//! `complete` or a disconnect causes, and every protocol close code the gateway owns.
//!
//! `runtime.rs` counts the tasks the gateway spawns; this file asserts only what crosses the
//! wire and what the engine is told. Every wait is under a real-clock deadline, on a loopback
//! port the server chooses.

use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use futures_util::{SinkExt, Stream, StreamExt, stream};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, Bound, BoxFuture, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal};
use ulo_graphql::{BoxStream, Engine, GqlError, GqlRequest, GqlResponse};
use ulo_graphql_ws::GraphqlWs;
use ulo_graphql_ws::__private::InitTimeout;
use ulo_runtime_tokio::Tokio;
use ulo_ws::WsModule;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(5);

/// What the engine was asked and what became of its streams.
#[derive(Default)]
struct Seen {
    subscribed: Mutex<Vec<GqlRequest>>,
    /// Streams the engine handed out that are still alive.
    open: AtomicUsize,
    dropped: AtomicUsize,
    /// The execution each `subscribe` was given, in the order the engine saw them, so a test can
    /// read why one was cancelled.
    execs: Mutex<Vec<ExecutionRef>>,
    /// `block` queries wait in the engine for this to be released; `entered` counts them arriving.
    gate: Gate,
}

/// A latch a test opens; a waiter gives up after the real-clock deadline rather than hang.
#[derive(Default)]
struct Gate {
    entered: AtomicUsize,
    open: Mutex<bool>,
    opened: std::sync::Condvar,
}

impl Gate {
    /// Blocks the calling worker thread. `block_in_place` hands the worker's I/O and timer driver
    /// to another thread first, so the rest of the app, and the test, keep running; it needs the
    /// multi-thread runtime.
    fn wait(&self) {
        self.entered.fetch_add(1, Ordering::SeqCst);
        tokio::task::block_in_place(|| {
            let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
            let deadline = std::time::Instant::now() + WAIT;
            while !*open {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!left.is_zero(), "the gate was never opened");
                open = self.opened.wait_timeout(open, left).unwrap_or_else(PoisonError::into_inner).0;
            }
        });
    }

    fn release(&self) {
        *self.open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.opened.notify_all();
    }
}

impl Seen {
    fn reasons(&self) -> Vec<Option<ulo::CancelReason>> {
        self.execs.lock().unwrap_or_else(PoisonError::into_inner).iter().map(ExecutionRef::cancel_reason).collect()
    }

    fn subscribed(&self) -> Vec<GqlRequest> {
        self.subscribed.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

/// Counts its own drop, which is how the gateway ending an operation reaches the engine.
struct Counted {
    inner: BoxStream<'static, GqlResponse>,
    seen: Arc<Seen>,
}

impl Stream for Counted {
    type Item = GqlResponse;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<GqlResponse>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl Drop for Counted {
    fn drop(&mut self) {
        self.seen.open.fetch_sub(1, Ordering::SeqCst);
        self.seen.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

/// Answers by what the query names: `ticks` two events, `hang` none and never ends,
/// `requestError` and `failed` one response from before execution, anything else one response.
struct Scripted(Arc<Seen>);

impl Engine for Scripted {
    fn execute(&self, _req: GqlRequest, _exec: ExecutionRef) -> BoxFuture<'static, GqlResponse> {
        Box::pin(async { GqlResponse::executed(Value::Null, Vec::new()) })
    }

    fn subscribe(&self, req: GqlRequest, exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        let query = req.query.clone();
        self.0.subscribed.lock().unwrap_or_else(PoisonError::into_inner).push(req);
        self.0.execs.lock().unwrap_or_else(PoisonError::into_inner).push(exec);
        self.0.open.fetch_add(1, Ordering::SeqCst);
        if query.contains("block") {
            self.0.gate.wait();
        }
        let inner: BoxStream<'static, GqlResponse> = if query.contains("hang") || query.contains("block") {
            Box::pin(stream::pending())
        } else if query.contains("requestError") {
            Box::pin(stream::iter([GqlResponse::request_error(vec![GqlError::new("the document is invalid")])]))
        } else if query.contains("failed") {
            Box::pin(stream::iter([GqlResponse::failed(vec![GqlError::new("the server failed")])]))
        } else if query.contains("ticks") {
            Box::pin(stream::iter((1..=2).map(|tick| GqlResponse::executed(json!({ "tick": tick }), Vec::new()))))
        } else {
            Box::pin(stream::iter([GqlResponse::executed(json!({ "one": true }), Vec::new())]))
        };
        Box::pin(Counted { inner, seen: Arc::clone(&self.0) })
    }

    fn sdl(&self) -> String {
        String::new()
    }

    fn playground_html(&self, _endpoint: &str, _subscriptions: Option<&str>) -> Option<String> {
        None
    }
}

struct Root(Arc<Seen>);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(Scripted(Arc::clone(&self.0))).also_as::<dyn Engine>(|engine| engine);
        // The init timeout is `runtime.rs`'s; here no connection closes for waiting.
        m.value(InitTimeout(Bound::Unbounded));
        m.controller::<GraphqlWs>().at("/graphql");
    }
}

struct Running {
    addr: SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
    seen: Arc<Seen>,
}

async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("{what} did not happen within {WAIT:?}"))
}

async fn start() -> Running {
    let seen = Arc::new(Seen::default());
    let app: App<Serving> = App::builder(Root(Arc::clone(&seen)))
        .runtime(Tokio::current())
        .drain_timeout(Duration::from_secs(2))
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_http_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addr = app.addresses()[0].addr;
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Running { addr, handle, serving, seen }
}

impl Running {
    /// A connection offering `subprotocol`, or none at all.
    async fn connect_offering(&self, subprotocol: Option<&'static str>) -> Socket {
        let mut request = format!("ws://{}/graphql", self.addr).into_client_request().expect("a client request");
        if let Some(subprotocol) = subprotocol {
            request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static(subprotocol));
        }
        let (socket, _) = within("the upgrade", tokio_tungstenite::connect_async(request)).await.expect("the upgrade succeeded");
        socket
    }

    async fn connect(&self) -> Socket {
        self.connect_offering(Some("graphql-transport-ws")).await
    }

    /// A connection that has sent `connection_init` and been acknowledged.
    async fn ready(&self) -> Socket {
        let mut socket = self.connect().await;
        send(&mut socket, json!({ "type": "connection_init" })).await;
        assert_eq!(next(&mut socket).await, json!({ "type": "connection_ack" }));
        socket
    }

    async fn stop(self) {
        within("the app's close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }

    /// Waits until `condition` holds of what the engine saw.
    async fn until(&self, what: &str, condition: impl Fn(&Seen) -> bool) {
        within(what, async {
            while !condition(&self.seen) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
    }
}

async fn send(socket: &mut Socket, message: Value) {
    socket.send(Message::text(message.to_string())).await.expect("the message was sent");
}

async fn next(socket: &mut Socket) -> Value {
    match within("the next message", socket.next()).await.expect("the connection ended").expect("a read failed") {
        Message::Text(text) => serde_json::from_str(text.as_str()).expect("a JSON message"),
        other => panic!("expected a text message, got {other:?}"),
    }
}

/// The close frame the server sends: its code and reason, with any frame before it skipped.
async fn closed_with(socket: &mut Socket) -> (u16, String) {
    loop {
        match within("the close frame", socket.next()).await {
            Some(Ok(Message::Close(Some(frame)))) => return (u16::from(frame.code), frame.reason.to_string()),
            Some(Ok(Message::Close(None))) => panic!("the server closed without a code"),
            Some(Ok(_)) => {}
            other => panic!("expected a close frame, got {other:?}"),
        }
    }
}

fn subscribe(id: &str, query: &str) -> Value {
    json!({ "type": "subscribe", "id": id, "payload": { "query": query } })
}

/// `connection_ack`, then each event as `next` carrying its operation's `id`, then `complete`;
/// the engine is handed the request exactly as the client sent it.
#[tokio::test]
async fn a_subscription_is_acknowledged_streams_next_and_ends_with_complete() {
    let app = start().await;
    let mut socket = app.ready().await;
    let payload = json!({ "query": "subscription Q($n: Int) { ticks }", "operationName": "Q", "variables": { "n": 1 }, "extensions": { "k": "v" } });
    send(&mut socket, json!({ "type": "subscribe", "id": "op-1", "payload": payload })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "next", "id": "op-1", "payload": { "data": { "tick": 1 } } }));
    assert_eq!(next(&mut socket).await, json!({ "type": "next", "id": "op-1", "payload": { "data": { "tick": 2 } } }));
    assert_eq!(next(&mut socket).await, json!({ "type": "complete", "id": "op-1" }));

    let seen = app.seen.subscribed();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0].query, "subscription Q($n: Int) { ticks }");
    assert_eq!(seen[0].operation_name.as_deref(), Some("Q"));
    assert_eq!(seen[0].variables.as_ref().map(|v| Value::Object(v.clone())), Some(json!({ "n": 1 })));
    assert_eq!(seen[0].extensions.as_ref().map(|v| Value::Object(v.clone())), Some(json!({ "k": "v" })));

    app.until("the finished operation's stream to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 1).await;
    let _ = socket.close(None).await;
    app.stop().await;
}

/// Two operations on one connection are told apart by `id`, each ending on its own.
#[tokio::test]
async fn operations_on_one_connection_are_told_apart_by_id_and_a_finished_id_is_free_again() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("a", "subscription { ticks }")).await;
    send(&mut socket, subscribe("b", "subscription { one }")).await;

    let mut by_id: std::collections::BTreeMap<String, Vec<Value>> = std::collections::BTreeMap::new();
    // 2 `next` and a `complete` for `a`; 1 `next` and a `complete` for `b`.
    for _ in 0..5 {
        let message = next(&mut socket).await;
        by_id.entry(message["id"].as_str().expect("an id").to_owned()).or_default().push(message);
    }
    let kinds = |id: &str| by_id[id].iter().map(|m| m["type"].as_str().unwrap_or_default().to_owned()).collect::<Vec<_>>();
    assert_eq!(kinds("a"), ["next", "next", "complete"]);
    assert_eq!(kinds("b"), ["next", "complete"]);

    send(&mut socket, subscribe("a", "subscription { ticks }")).await;
    assert_eq!(next(&mut socket).await["type"], "next", "an id whose operation completed starts a new one, and is not a duplicate");
    let _ = socket.close(None).await;
    app.stop().await;
}

/// A `subscribe` before `connection_init` was acknowledged closes the connection 4401.
#[tokio::test]
async fn a_subscribe_before_the_ack_closes_4401() {
    let app = start().await;
    let mut socket = app.connect().await;
    send(&mut socket, subscribe("early", "subscription { ticks }")).await;
    assert_eq!(closed_with(&mut socket).await, (4401, "Unauthorized".to_owned()));
    assert!(app.seen.subscribed().is_empty(), "the engine was never asked");
    app.stop().await;
}

/// A second `connection_init` closes 4429, and so does it on a connection that already runs an
/// operation, which it leaves to be cancelled with the connection.
#[tokio::test]
async fn a_second_connection_init_closes_4429() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, json!({ "type": "connection_init" })).await;
    assert_eq!(closed_with(&mut socket).await, (4429, "Too many initialisation requests".to_owned()));

    let mut busy = app.ready().await;
    send(&mut busy, subscribe("running", "subscription { hang }")).await;
    app.until("the operation to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 1).await;
    send(&mut busy, json!({ "type": "connection_init" })).await;
    assert_eq!(
        closed_with(&mut busy).await,
        (4429, "Too many initialisation requests".to_owned()),
        "a running operation does not spare the connection"
    );
    // The operation ends with the connection, which the client ends here.
    drop(busy);
    app.until("the running operation's stream to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 1).await;
    app.stop().await;
}

/// A `subscribe` under the `id` of a running operation closes the connection 4409.
#[tokio::test]
async fn a_duplicate_operation_id_closes_4409() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("same", "subscription { hang }")).await;
    app.until("the first operation to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 1).await;
    send(&mut socket, subscribe("same", "subscription { hang }")).await;
    // The reason the protocol gives 4409 names the id, the one way a client learns which.
    assert_eq!(closed_with(&mut socket).await, (4409, "Subscriber for same already exists".to_owned()));
    assert_eq!(app.seen.subscribed().len(), 1, "the second was never given to the engine");
    app.stop().await;
}

/// A message that is not one the protocol defines closes 4400: text that is not JSON, a `type`
/// the client side has none of, a `subscribe` with no `id`, and a binary frame.
#[tokio::test]
async fn a_message_the_protocol_does_not_define_closes_4400() {
    let app = start().await;
    let cases: [(&str, Message); 5] = [
        ("text that is not JSON", Message::text("not json")),
        ("an unknown type", Message::text(json!({ "type": "connection_ack" }).to_string())),
        ("a type the server sends", Message::text(json!({ "type": "next", "id": "x", "payload": {} }).to_string())),
        ("a subscribe with no id", Message::text(json!({ "type": "subscribe", "payload": { "query": "{ one }" } }).to_string())),
        ("a binary frame", Message::binary(vec![1, 2, 3])),
    ];
    for (what, message) in cases {
        let mut socket = app.connect().await;
        socket.send(message).await.expect("the frame was sent");
        assert_eq!(closed_with(&mut socket).await, (4400, "Invalid message received".to_owned()), "{what}");
    }
    app.stop().await;
}

/// `ping` is answered with `pong`, echoing the payload when it has one; a `pong` from the client
/// is accepted and answered with nothing.
#[tokio::test]
async fn ping_is_answered_with_pong_and_a_client_pong_with_nothing() {
    let app = start().await;
    let mut socket = app.connect().await;
    send(&mut socket, json!({ "type": "ping", "payload": { "n": 1 } })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "pong", "payload": { "n": 1 } }));
    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "pong" }));
    send(&mut socket, json!({ "type": "pong", "payload": { "ignored": true } })).await;
    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "pong" }), "the next message is the next ping's answer, nothing came for the pong");
    let _ = socket.close(None).await;
    app.stop().await;
}

/// A handshake that echoed no `graphql-transport-ws` is closed 4406. A client offering another
/// protocol is not given one: the server echoes none, and a conforming client abandons the
/// handshake itself.
#[tokio::test]
async fn a_connection_without_the_subprotocol_closes_4406() {
    let app = start().await;
    let mut socket = app.connect_offering(None).await;
    assert_eq!(closed_with(&mut socket).await, (4406, "Subprotocol not acceptable".to_owned()));

    let mut request = format!("ws://{}/graphql", app.addr).into_client_request().expect("a client request");
    request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static("graphql-ws"));
    let refused = within("the handshake", tokio_tungstenite::connect_async(request)).await;
    assert!(refused.is_err(), "no protocol was echoed for `graphql-ws`");
    app.stop().await;
}

/// A request that failed before execution is one `error` message carrying the errors as a list,
/// which ends the operation with no `complete`; a request the server failed is the same.
#[tokio::test]
async fn a_request_that_failed_before_execution_is_an_error_message_with_no_complete() {
    let app = start().await;
    let mut socket = app.ready().await;
    // The payload is the engine's error list as the engine wrote it.
    let cases = [("bad", "subscription { requestError }", "the document is invalid"), ("broken", "subscription { failed }", "the server failed")];
    for (id, query, engine_said) in cases {
        send(&mut socket, subscribe(id, query)).await;
        assert_eq!(next(&mut socket).await, json!({ "type": "error", "id": id, "payload": [{ "message": engine_said }] }));
    }
    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "pong" }), "no `complete` followed either `error`");
    app.until("both operations' streams to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 2).await;
    let _ = socket.close(None).await;
    app.stop().await;
}

/// The client's `complete` cancels the operation: the engine's stream is dropped, the server
/// sends nothing for it, and the id may be used again.
#[tokio::test]
async fn a_client_complete_cancels_the_operation_and_frees_its_id() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("c", "subscription { hang }")).await;
    app.until("the operation to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 1).await;
    send(&mut socket, json!({ "type": "complete", "id": "c" })).await;
    app.until("the cancelled operation's stream to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 1).await;

    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "pong" }), "nothing was sent for the cancelled operation");
    send(&mut socket, subscribe("c", "subscription { ticks }")).await;
    assert_eq!(next(&mut socket).await["type"], "next", "the id is free");
    let _ = socket.close(None).await;
    app.stop().await;
}

/// A `complete` for an id that is not running is ignored.
#[tokio::test]
async fn a_complete_for_an_unknown_id_is_ignored() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, json!({ "type": "complete", "id": "nobody" })).await;
    send(&mut socket, json!({ "type": "ping" })).await;
    assert_eq!(next(&mut socket).await, json!({ "type": "pong" }));
    let _ = socket.close(None).await;
    app.stop().await;
}

/// A connection that goes away cancels every operation it was running.
#[tokio::test]
async fn a_disconnect_cancels_every_running_operation() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("one", "subscription { hang }")).await;
    send(&mut socket, subscribe("two", "subscription { hang }")).await;
    app.until("both operations to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 2).await;
    drop(socket);
    app.until("both streams to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 2).await;
    assert_eq!(app.seen.open.load(Ordering::SeqCst), 0);
    app.stop().await;
}

/// The app's drain ends a running operation with `complete`, so the client learns the server
/// stopped it.
#[tokio::test]
async fn the_drain_ends_a_running_operation_with_complete() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("long", "subscription { hang }")).await;
    app.until("the operation to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 1).await;
    let handle = app.handle.clone();
    let closing = tokio::spawn(async move {
        let _ = handle.close(Signal::new("test")).await;
    });
    assert_eq!(next(&mut socket).await, json!({ "type": "complete", "id": "long" }));
    within("the close", closing).await.expect("the close task ended");
    let _ = within("serve", app.serving).await;
}

/// The gateway's own close ends every operation the connection runs, without waiting for the
/// client to answer the close frame: the engine's stream is dropped and the execution reads
/// `Explicit`, not the `Disconnected` the connection's end would write later. The client here
/// reads the close frame and then keeps the socket.
#[tokio::test]
async fn the_gateways_own_close_cancels_every_running_operation_with_explicit() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("one", "subscription { hang }")).await;
    send(&mut socket, subscribe("two", "subscription { hang }")).await;
    app.until("both operations to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 2).await;

    send(&mut socket, json!({ "type": "connection_init" })).await;
    assert_eq!(closed_with(&mut socket).await, (4429, "Too many initialisation requests".to_owned()));
    // The socket is still held: only the gateway's close can have cancelled the operations.
    app.until("both streams to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 2).await;
    let cancelled = ulo::CancelReason::Explicit;
    assert_eq!(app.seen.reasons(), [Some(cancelled), Some(cancelled)], "one reason per operation, in subscribe order");
    drop(socket);
    app.stop().await;
}

/// The same for each close the gateway sends with operations running: a duplicate id takes the
/// first operation with it, and a message the protocol does not define takes all of them.
#[tokio::test]
async fn a_duplicate_id_and_an_invalid_message_cancel_the_running_operations_with_explicit() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("same", "subscription { hang }")).await;
    app.until("the operation to reach the engine", |seen| seen.open.load(Ordering::SeqCst) == 1).await;
    send(&mut socket, subscribe("same", "subscription { hang }")).await;
    assert_eq!(closed_with(&mut socket).await.0, 4409);
    app.until("the first stream to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 1).await;
    assert_eq!(app.seen.reasons(), [Some(ulo::CancelReason::Explicit)], "the duplicate close");

    let mut other = app.ready().await;
    send(&mut other, subscribe("running", "subscription { hang }")).await;
    app.until("the second connection's operation", |seen| seen.open.load(Ordering::SeqCst) == 1).await;
    other.send(Message::text("not json")).await.expect("the frame was sent");
    assert_eq!(closed_with(&mut other).await.0, 4400);
    app.until("the second stream to be dropped", |seen| seen.dropped.load(Ordering::SeqCst) == 2).await;
    assert_eq!(app.seen.reasons(), [Some(ulo::CancelReason::Explicit), Some(ulo::CancelReason::Explicit)], "the invalid-message close");
    drop((socket, other));
    app.stop().await;
}

/// A `subscribe` the gateway reads before the drain and handles after it is refused with an
/// `error` message carrying the shutdown phrase, and never reaches the engine; the operation that
/// was running ends with `complete`. The engine holds the first `subscribe` so that the second,
/// already read, is handled once the drain has begun.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_subscribe_handled_during_the_drain_is_refused_with_the_shutdown_phrase() {
    let app = start().await;
    let mut socket = app.ready().await;
    send(&mut socket, subscribe("held", "subscription { block }")).await;
    send(&mut socket, subscribe("late", "subscription { one }")).await;
    within("the engine to hold the first subscribe", async {
        while app.seen.gate.entered.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    // The second frame is read while the first is held; nothing is read once the drain begins.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let handle = app.handle.clone();
    let closing = tokio::spawn(async move {
        let _ = handle.close(Signal::new("test")).await;
    });
    within("the app turning to draining", async {
        while !app.handle.is_draining() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    app.seen.gate.release();

    let mut messages = Vec::new();
    let closed = loop {
        match within("the next frame", socket.next()).await {
            Some(Ok(Message::Text(text))) => messages.push(serde_json::from_str::<Value>(text.as_str()).expect("a JSON message")),
            Some(Ok(Message::Close(Some(frame)))) => break (u16::from(frame.code), frame.reason.to_string()),
            other => panic!("expected a message or the close frame, got {other:?}"),
        }
    };
    let refused = json!({ "type": "error", "id": "late", "payload": [{ "message": "the server is shutting down" }] });
    assert_eq!(messages.iter().filter(|message| **message == refused).count(), 1, "the refusal of `late` among {messages:?}");
    assert!(messages.contains(&json!({ "type": "complete", "id": "held" })), "the held operation ended with complete: {messages:?}");
    assert_eq!(messages.len(), 2, "nothing else was sent: {messages:?}");
    assert_eq!(closed, (1001, "the server is shutting down".to_owned()), "the drain's close");
    assert_eq!(app.seen.subscribed().len(), 1, "`late` never reached the engine");
    within("the close", closing).await.expect("the close task ended");
    let _ = within("serve", app.serving).await;
}
