//! The `connection_init_timeout` watch waits on the app's timer and runs on the app's runtime, so
//! a timer that has shut down while the runtime still spawns refuses the wait. The connection then
//! closes with the shutdown's 1001: no time was measured, and 4408 claims the init was late.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use ulo::app::Bound as Serving;
use ulo::{App, AppHandle, Bound, BoxFuture, ExecutionRef, Module, ModuleDef, ModuleIdentity, Runtime, Signal, Spawn, TaskHandle, Timer};
use ulo_conformance_support::ScriptedTimer;
use ulo_graphql::{BoxStream, Engine, GqlRequest, GqlResponse};
use ulo_graphql_ws::GraphqlWs;
use ulo_graphql_ws::__private::InitTimeout;
use ulo_runtime_tokio::Tokio;
use ulo_ws::WsModule;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const WAIT: Duration = Duration::from_secs(5);

/// Unlike every other length the app arms: the keep-alive's 30 seconds, the drain's 2.
const INIT: Duration = Duration::from_secs(3);

/// A scripted clock with tokio's spawning.
#[derive(Clone)]
struct Handed {
    clock: ScriptedTimer,
    spawn: Tokio,
}

impl Timer for Handed {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        Timer::try_sleep(&self.clock, d)
    }
}

impl Spawn for Handed {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.spawn.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        Spawn::try_spawn(&self.spawn, fut)
    }
}

/// [`Handed`] whose wait of `INIT` is refused once [`Held::release`] is called, so the refusal
/// arrives after the connection was acknowledged.
#[derive(Clone)]
struct Held {
    handed: Handed,
    gate: Arc<Gate>,
}

#[derive(Default)]
struct Gate {
    released: AtomicBool,
    refused: AtomicBool,
    waiting: Mutex<Vec<Waker>>,
}

impl Held {
    fn release(&self) {
        self.gate.released.store(true, Ordering::SeqCst);
        self.gate.waiting.lock().unwrap_or_else(PoisonError::into_inner).drain(..).for_each(Waker::wake);
    }
}

impl Timer for Held {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.handed.sleep(d)
    }

    fn now(&self) -> Instant {
        self.handed.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        if d != INIT {
            return self.handed.try_sleep(d);
        }
        let gate = Arc::clone(&self.gate);
        Box::pin(std::future::poll_fn(move |cx| {
            gate.waiting.lock().unwrap_or_else(PoisonError::into_inner).push(cx.waker().clone());
            if gate.released.load(Ordering::SeqCst) {
                gate.refused.store(true, Ordering::SeqCst);
                return Poll::Ready(Err(ulo::Refused));
            }
            Poll::Pending
        }))
    }
}

impl Spawn for Held {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.handed.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        self.handed.try_spawn(fut)
    }
}

struct Idle;

impl Engine for Idle {
    fn execute(&self, _req: GqlRequest, _exec: ExecutionRef) -> BoxFuture<'static, GqlResponse> {
        Box::pin(async { GqlResponse::executed(Value::Null, Vec::new()) })
    }

    fn subscribe(&self, _req: GqlRequest, _exec: ExecutionRef) -> BoxStream<'static, GqlResponse> {
        Box::pin(futures_util::stream::empty())
    }

    fn sdl(&self) -> String {
        String::new()
    }

    fn playground_html(&self, _endpoint: &str, _subscriptions: Option<&str>) -> Option<String> {
        None
    }
}

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(Idle).also_as::<dyn Engine>(|engine| engine);
        m.value(InitTimeout(Bound::After(INIT)));
        m.controller::<GraphqlWs>().at("/graphql");
    }
}

struct Running {
    addr: SocketAddr,
    handle: AppHandle,
    serving: tokio::task::JoinHandle<()>,
}

async fn start(clock: ScriptedTimer) -> Running {
    start_on(Handed { clock, spawn: Tokio::current() }).await
}

async fn start_on(runtime: impl Runtime) -> Running {
    let app: App<Serving> = App::builder(Root)
        .runtime(runtime)
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
    Running { addr, handle, serving }
}

impl Running {
    async fn connect(&self) -> Socket {
        let mut request = format!("ws://{}/graphql", self.addr).into_client_request().expect("a client request");
        request.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_static("graphql-transport-ws"));
        let (socket, _) = within("the upgrade", tokio_tungstenite::connect_async(request)).await.expect("the upgrade succeeded");
        socket
    }

    async fn stop(self) {
        within("the app's close", async {
            let _ = self.handle.close(Signal::new("test")).await;
            let _ = self.serving.await;
        })
        .await;
    }
}

async fn within<F: Future>(what: &str, fut: F) -> F::Output {
    tokio::time::timeout(WAIT, fut).await.unwrap_or_else(|_| panic!("{what} did not happen within {WAIT:?}"))
}

/// The code and reason of the Close frame the server ends `socket` with.
async fn close_frame(socket: &mut Socket) -> (u16, String) {
    loop {
        match within("the close", socket.next()).await {
            Some(Ok(Message::Close(Some(frame)))) => break (u16::from(frame.code), frame.reason.as_str().to_owned()),
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("expected a close frame, got {other:?}"),
        }
    }
}

/// Answers the server's Close and reads until the server ends the connection.
async fn hang_up(mut socket: Socket) {
    let _ = socket.close(None).await;
    within("the server's end of the connection", async { while let Some(Ok(_)) = socket.next().await {} }).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_init_wait_closes_1001_with_the_shutdown_phrase() {
    let clock = ScriptedTimer::new();
    clock.refuse(INIT);
    let app = start(clock.clone()).await;
    let mut socket = app.connect().await;
    assert_eq!(close_frame(&mut socket).await, (1001, ulo_transport::SHUTDOWN_MESSAGE.to_owned()), "the close of an init wait the timer refused");
    assert_eq!(clock.refused(INIT), 1, "the init waits asked of a timer refusing them");
    hang_up(socket).await;
    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_init_wait_that_elapses_closes_4408() {
    let clock = ScriptedTimer::new();
    let app = start(clock.clone()).await;
    let mut socket = app.connect().await;
    within("the init wait", async {
        while clock.live(INIT) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    clock.advance(INIT);
    assert_eq!(close_frame(&mut socket).await.0, 4408, "the close of an init that did not arrive in time");
    hang_up(socket).await;
    app.stop().await;
}

/// The control for the acknowledged case: a refusal that arrives once `connection_init` was
/// acknowledged closes nothing, and the connection keeps answering.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_init_wait_after_the_ack_closes_nothing() {
    let held = Held { handed: Handed { clock: ScriptedTimer::new(), spawn: Tokio::current() }, gate: Arc::default() };
    let app = start_on(held.clone()).await;
    let mut socket = app.connect().await;
    let send = |message: Value| Message::text(message.to_string());
    within("the init", socket.send(send(json!({ "type": "connection_init" })))).await.expect("the init was sent");
    assert_eq!(next_type(&mut socket).await, "connection_ack");
    held.release();
    within("the refusal", async {
        while !held.gate.refused.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    // Room for a close the watch would send to arrive ahead of the pong.
    tokio::time::sleep(Duration::from_millis(50)).await;
    within("the ping", socket.send(send(json!({ "type": "ping" })))).await.expect("the ping was sent");
    assert_eq!(next_type(&mut socket).await, "pong", "an acknowledged connection whose init wait was refused stopped answering");
    hang_up(socket).await;
    app.stop().await;
}

/// The `type` of the next text message the server sends.
async fn next_type(socket: &mut Socket) -> String {
    loop {
        match within("the next message", socket.next()).await {
            Some(Ok(Message::Text(text))) => {
                let message: Value = serde_json::from_str(text.as_str()).expect("the server sent JSON");
                break message["type"].as_str().unwrap_or_default().to_owned();
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
            other => panic!("expected a text message, got {other:?}"),
        }
    }
}
