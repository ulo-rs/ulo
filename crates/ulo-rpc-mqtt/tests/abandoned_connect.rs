//! F369: a `connect` or a `listen` dropped while the broker has not yet answered takes its event
//! loop with it, and so does one its `connect_within` cuts short (F642). The broker here is a
//! socket that reads the CONNECT, waits for the drop or the cut, then answers CONNACK: a loop left
//! running would go on to SUBSCRIBE, and an ended one has closed the connection.
//!
//! The cut is timed on a scripted clock: each test waits until the link has armed a wait of
//! exactly its bound, moves the clock past it, and the connect fails at once with
//! `ConnectTimedOut`, before rumqttc's own five-second connection timeout could end it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use ulo::testing::ScriptedTimer;
use ulo::{BoxError, BoxFuture, Refused, Runtime, Spawn, TaskHandle, Timer};
use ulo_rpc::{ConnectTimedOut, Link, Pattern};
use ulo_rpc_mqtt::Mqtt;

/// The runtime a client hands its link: the test's tokio runtime, which a tokio link ignores.
fn client_runtime() -> Arc<dyn Runtime> {
    Arc::new(ulo_runtime_tokio::Tokio::current())
}

/// How long the broker waits for each step of the client.
const PATIENCE: Duration = Duration::from_secs(5);

/// An MQTT 5 CONNACK: no session present, reason code success, no properties.
const CONNACK: [u8; 5] = [0x20, 0x03, 0x00, 0x00, 0x00];

const CONNECT: u8 = 1;
const SUBSCRIBE: u8 = 8;

/// Accepts the link's connection and reads its first packet, which is a CONNECT.
async fn handshake_begun(listener: &TcpListener) -> TcpStream {
    let (mut stream, _) = tokio::time::timeout(PATIENCE, listener.accept())
        .await
        .expect("the link did not connect within the patience")
        .expect("the broker's socket accepts");
    let mut buffer = [0u8; 512];
    let read = tokio::time::timeout(PATIENCE, stream.read(&mut buffer))
        .await
        .expect("the link sent nothing within the patience")
        .expect("the broker's socket reads");
    assert!(read > 0 && buffer[0] >> 4 == CONNECT, "the link's first packet is not a CONNECT: {:02x?}", &buffer[..read]);
    stream
}

/// Answers the CONNECT once the link's future has been dropped, then requires the connection to
/// end with no SUBSCRIBE on it.
async fn ended_unanswered(mut stream: TcpStream, what: &str) {
    // A closed connection may refuse the write; what is read afterwards decides.
    let _ = stream.write_all(&CONNACK).await;
    let mut buffer = [0u8; 512];
    loop {
        match tokio::time::timeout(PATIENCE, stream.read(&mut buffer)).await {
            Ok(Ok(0) | Err(_)) => return,
            Ok(Ok(read)) => {
                let kind = buffer[0] >> 4;
                assert_ne!(kind, SUBSCRIBE, "the event loop of a dropped `{what}` went on to subscribe: {:02x?}", &buffer[..read]);
            }
            Err(_) => panic!("the connection of a dropped `{what}` was still open {PATIENCE:?} after the broker answered"),
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connect_dropped_before_the_broker_answers_ends_its_event_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address")));
    let connecting = tokio::spawn(async move { link.connect(&client_runtime()).await.map(drop) });
    let stream = handshake_begun(&listener).await;
    connecting.abort();
    let dropped: Result<Result<(), BoxError>, _> = connecting.await;
    assert!(dropped.is_err_and(|join| join.is_cancelled()), "the connect finished before the broker answered");
    ended_unanswered(stream, "connect").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listen_dropped_before_the_broker_answers_ends_its_event_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let mut link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address"))).group("abandoned");
    let app = ulo::App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    link.prepare(&app.handle()).await.expect("the link prepares");
    let listening = tokio::spawn(async move { link.listen(&[Pattern::from("abandoned.call").into()]).await.map(drop) });
    let stream = handshake_begun(&listener).await;
    listening.abort();
    let dropped: Result<Result<(), BoxError>, _> = listening.await;
    assert!(dropped.is_err_and(|join| join.is_cancelled()), "the listen finished before the broker answered");
    ended_unanswered(stream, "listen").await;
}

struct Root;

impl ulo::Module for Root {
    fn identity(&self) -> ulo::ModuleIdentity {
        ulo::ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ulo::ModuleDef<'_>) {
        let _ = m;
    }
}

/// The bound the cut tests set, a length the scripted clock alone times.
const WITHIN: Duration = Duration::from_secs(3);

/// The test's tokio runtime with a scripted clock, as a client is handed it.
struct Clocked {
    tokio: ulo_runtime_tokio::Tokio,
    clock: ScriptedTimer,
}

impl Timer for Clocked {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.clock.sleep(d)
    }

    fn now(&self) -> Instant {
        self.clock.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), Refused>> {
        self.clock.try_sleep(d)
    }
}

impl Spawn for Clocked {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.tokio.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, Refused> {
        self.tokio.try_spawn(fut)
    }

    fn spawn_blocking(&self, work: Box<dyn FnOnce() + Send>) -> TaskHandle {
        self.tokio.spawn_blocking(work)
    }
}

/// Waits in real time until a wait of exactly `WITHIN` is live on `clock`, then moves the clock
/// past it.
async fn cut(clock: &ScriptedTimer, what: &str) {
    let began = Instant::now();
    while clock.live(WITHIN) == 0 {
        assert!(began.elapsed() < PATIENCE, "{what} armed no wait of `connect_within` within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    clock.advance(WITHIN);
}

fn timed_out(outcome: Result<(), BoxError>, what: &str) {
    let refused = outcome.err().unwrap_or_else(|| panic!("{what} connected to a broker that never answered"));
    let timed_out = refused.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("{what} failed other than as `ConnectTimedOut`: {refused}"));
    assert_eq!((timed_out.within(), timed_out.link()), (WITHIN, "MQTT"), "{what}: {refused}");
    assert!(timed_out.address().starts_with("127.0.0.1:"), "{what} names the broker by its address: {refused}");
}

/// Planted breaks: `listen` waiting for the broker with no bound of its own, which rumqttc ends
/// at five seconds with its own `Timeout`; the bound's expiry leaving the handshake armed.
#[tokio::test(flavor = "multi_thread")]
async fn a_listen_cut_at_connect_within_ends_its_event_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let mut link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address")))
        .group("abandoned")
        .connect_within(WITHIN);
    let clock = ScriptedTimer::new();
    let app = ulo::App::builder(Root)
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .wire()
        .expect("the app wires")
        .connect()
        .await
        .expect("the app connects");
    link.prepare(&app.handle()).await.expect("the link prepares");
    let listening = tokio::spawn(async move { link.listen(&[Pattern::from("abandoned.call").into()]).await.map(drop) });
    let stream = handshake_begun(&listener).await;
    cut(&clock, "`listen`").await;
    let outcome = tokio::time::timeout(PATIENCE, listening).await.expect("`listen` ended at its bound").expect("the listen task ran");
    timed_out(outcome, "`listen`");
    ended_unanswered(stream, "listen cut at `connect_within`").await;
}

/// Planted break: a client's connect waiting for the broker with no bound of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_connect_cut_at_connect_within_ends_its_event_loop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("the broker's socket binds");
    let link = Mqtt::url(format!("mqtt://{}", listener.local_addr().expect("the broker's socket has an address"))).connect_within(WITHIN);
    let clock = ScriptedTimer::new();
    let runtime: Arc<dyn Runtime> = Arc::new(Clocked { tokio: ulo_runtime_tokio::Tokio::current(), clock: clock.clone() });
    let connecting = tokio::spawn(async move { link.connect(&runtime).await.map(drop) });
    let stream = handshake_begun(&listener).await;
    cut(&clock, "the client's connect").await;
    let outcome = tokio::time::timeout(PATIENCE, connecting).await.expect("the connect ended at its bound").expect("the connect task ran");
    timed_out(outcome, "the client's connect");
    ended_unanswered(stream, "connect cut at `connect_within`").await;
}

/// A zero bound is refused where a client takes the link and in a server's `prepare`, before any
/// I/O. Planted break: `usable` not reading `connect_within`.
#[tokio::test(flavor = "current_thread")]
async fn a_zero_connect_within_is_refused_before_any_io() {
    let mut zero = Mqtt::url("mqtt://127.0.0.1:1").group("zero").connect_within(Duration::ZERO);
    let refused = zero.usable().expect_err("`usable` accepted a zero `connect_within`");
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");
    let app = ulo::App::builder(Root).runtime(ulo_runtime_tokio::Tokio::current()).wire().expect("the app wires").connect().await.expect("the app connects");
    let refused = zero.prepare(&app.handle()).await.expect_err("`prepare` accepted a zero `connect_within`");
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");
}
