//! The answers a connection's last messages queue as the drain closes it are written before its
//! close frame, each once, in the order they were queued (F463). A message task can queue its
//! answer between the read loop's `take` and its decision to close; that decision follows a write
//! that may wait on the client, so several answers can land in the one window.
//!
//! Planted breaks: the close discarding what was queued (`shut`); the queued answers written in
//! reverse; the queued answers written twice.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::sync::Semaphore;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, Dep, ExecutionRef, Module, ModuleDef, ModuleIdentity, Signal, injectable, routes};
use ulo_ws::Payload;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const SHUTDOWN_PHRASE: &str = ulo_transport::SHUTDOWN_MESSAGE;

/// The longest any one wait lasts before it fails the test.
const WAIT: Duration = Duration::from_secs(10);

async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    match tokio::time::timeout(WAIT, fut).await {
        Ok(output) => output,
        Err(_) => panic!("{what} did not happen within {WAIT:?}"),
    }
}

/// What the handlers report back: how many entered, and which held messages finished.
#[derive(Clone, Default)]
struct Seen {
    entered: Arc<AtomicUsize>,
    finished: Arc<AtomicUsize>,
}

/// One gate per held message, opened by the test.
#[derive(Clone)]
struct Gates(Arc<Vec<Semaphore>>);

#[derive(Deserialize)]
struct Held {
    gate: usize,
    size: usize,
}

#[injectable]
struct Late {
    seen: Dep<Seen>,
    gates: Dep<Gates>,
}

#[routes]
#[ulo_ws::gateway(path = "/late", port = own)]
impl Late {
    /// Waits for its execution's cancellation and answers in the poll that wakes it.
    #[ulo_ws::message("observe")]
    async fn observe(&self, exec: ExecutionRef) -> String {
        self.seen.entered.fetch_add(1, Ordering::SeqCst);
        exec.cancelled().await;
        format!("observed {:?}", exec.cancel_reason())
    }

    /// Waits for its gate and answers `size` bytes.
    #[ulo_ws::message("held")]
    async fn held(&self, held: Payload<Held>) -> String {
        self.seen.entered.fetch_add(1, Ordering::SeqCst);
        self.gates.0[held.0.gate].acquire().await.expect("the gate is never closed").forget();
        self.seen.finished.fetch_add(1, Ordering::SeqCst);
        "x".repeat(held.0.size)
    }
}

struct Root {
    seen: Seen,
    gates: Gates,
}

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.seen.clone());
        m.value(self.gates.clone());
        m.controller::<Late>();
    }
}

struct Running {
    addr: SocketAddr,
    handle: ulo::AppHandle,
    seen: Seen,
    gates: Gates,
    serving: tokio::task::JoinHandle<()>,
}

async fn start(drain: Duration) -> Running {
    let seen = Seen::default();
    let gates = Gates(Arc::new((0..3).map(|_| Semaphore::new(0)).collect()));
    let app: App<Serving> = App::builder(Root { seen: seen.clone(), gates: gates.clone() })
        .runtime(ulo_runtime_tokio::Tokio::current())
        .drain_timeout(drain)
        .wire()
        .unwrap_or_else(|error| panic!("the gateway app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the gateway app did not connect: {error}"))
        .bind(ulo_ws_hyper::Server::new("127.0.0.1:0"))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the gateway app did not listen: {error}"));
    let addr = match app.addresses().as_slice() {
        [bound] => bound.addr,
        other => panic!("expected one bound address, got {other:?}"),
    };
    let handle = app.handle();
    let serving = tokio::spawn(async move {
        let _ = app.serve(std::future::pending::<Signal>()).await;
    });
    Running { addr, handle, seen, gates, serving }
}

impl Running {
    async fn connect(&self) -> Socket {
        let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{}/late", self.addr)).await.expect("the WebSocket upgrade");
        socket
    }

    async fn entered(&self, n: usize) {
        within("the handlers entering", async {
            while self.seen.entered.load(Ordering::SeqCst) < n {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
    }

    /// Starts the close and returns once the app reports draining.
    async fn begin_close(&self) -> tokio::task::JoinHandle<()> {
        let handle = self.handle.clone();
        let closing = tokio::spawn(async move {
            let _ = handle.close(Signal::new("late")).await;
        });
        within("the app turning to draining", async {
            while !self.handle.is_draining() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        closing
    }
}

/// Every frame the socket carries up to and including its close: the text frames parsed, then the
/// close code and reason, or `None` when the stream ended without one.
async fn frames(mut socket: Socket) -> (Vec<Value>, Option<(u16, String)>) {
    let mut texts = Vec::new();
    loop {
        match socket.next().await {
            Some(Ok(Message::Text(text))) => texts.push(serde_json::from_str(text.as_str()).expect("a JSON frame")),
            Some(Ok(Message::Close(frame))) => {
                return (texts, frame.map(|frame| (u16::from(frame.code), frame.reason.to_string())));
            }
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return (texts, None),
        }
    }
}

/// Twenty apps, fifty connections each, every connection's two messages answering in the poll the
/// drain's end wakes them in. The first answer wakes the loop, and the second is queued while the
/// loop writes the first, so it races the close decision. Each connection reads both answers, each
/// once, and then 1001. The two wake in an order the drain picks, so the order is not asserted.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn each_answer_queued_at_the_close_arrives_once_before_1001() {
    let mut wrong = Vec::new();
    for app_index in 0..20 {
        let app = start(Duration::from_millis(150)).await;
        let mut sockets = Vec::new();
        for _ in 0..50 {
            let mut socket = app.connect().await;
            for id in [1, 2] {
                socket.send(Message::text(json!({ "event": "observe", "id": id }).to_string())).await.expect("the message is sent");
            }
            sockets.push(socket);
        }
        app.entered(100).await;
        let closing = app.begin_close().await;
        let reads: Vec<_> = sockets.into_iter().map(|socket| tokio::spawn(frames(socket))).collect();
        for (conn, read) in reads.into_iter().enumerate() {
            let (mut texts, close) = within("a connection's frames", read).await.expect("the reading task");
            texts.sort_by_key(|text| text["id"].as_u64());
            let answers = vec![json!({ "id": 1, "data": "observed Some(Drain)" }), json!({ "id": 2, "data": "observed Some(Drain)" })];
            if (&texts, &close) != (&answers, &Some((1001, SHUTDOWN_PHRASE.to_owned()))) {
                wrong.push(format!("app {app_index} connection {conn}: {texts:?} then {close:?}"));
            }
        }
        within("the close", closing).await.expect("the closing task");
        let _ = within("serve", app.serving).await;
    }
    assert!(wrong.is_empty(), "{} of 1000 connections read something else; the first: {:?}", wrong.len(), &wrong[..wrong.len().min(5)]);
}

/// The client reads nothing while the first answer, 12 MiB, is written, so once its frame reaches
/// the client the loop waits in that write with the drain begun; the second and third answers are queued during the wait and the
/// two handlers finish, leaving no message in flight. When the client reads, the loop decides to
/// close and writes both before the 1001, in the order they were queued.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_answers_queued_in_one_close_window_arrive_in_order_before_1001() {
    const BIG: usize = 12 * 1024 * 1024;
    let app = start(Duration::from_secs(5)).await;
    let mut socket = app.connect().await;
    for (id, gate, size) in [(1, 0, BIG), (2, 1, 2), (3, 2, 3)] {
        let message = json!({ "event": "held", "id": id, "data": { "gate": gate, "size": size } });
        socket.send(Message::text(message.to_string())).await.expect("the message is sent");
    }
    app.entered(3).await;
    let closing = app.begin_close().await;

    // The big answer first, released until its frame reaches the client's socket, which the loop
    // writes only after taking it: from then on the loop waits in that write. Then the two small
    // ones, one after the other.
    app.gates.0[0].add_permits(1);
    let MaybeTlsStream::Plain(tcp) = socket.get_ref() else { panic!("a plain TCP socket") };
    within("the first answer's frame reaching the client", tcp.peek(&mut [0u8; 1])).await.expect("the peek");
    for gate in [1, 2] {
        app.gates.0[gate].add_permits(1);
        within("the held handler finishing", async {
            while app.seen.finished.load(Ordering::SeqCst) < gate + 1 {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let (texts, close) = within("the connection's frames", frames(socket)).await;
    let read: Vec<(Value, usize)> = texts
        .iter()
        .map(|text| (text["id"].clone(), text["data"].as_str().map_or(usize::MAX, str::len)))
        .collect();
    assert_eq!(read, vec![(json!(1), BIG), (json!(2), 2), (json!(3), 3)], "the answers and their order");
    assert_eq!(close, Some((1001, SHUTDOWN_PHRASE.to_owned())), "the close after the answers");
    within("the close", closing).await.expect("the closing task");
    let _ = within("serve", app.serving).await;
}
