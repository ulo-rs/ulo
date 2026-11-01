//! A connection's keep-alive is timed on the app's runtime. A runtime that has shut down refuses
//! the next ping tick or the wait for a Pong, and the connection closes with 1001 and the shutdown
//! phrase, ending as `RuntimeShutDown`: on `sleep`, which ends at once on such a runtime, the
//! connection would ping without pause, or end as `Lost` for a Pong nobody waited for.
//!
//! A refusal of either wait that arrives once the Pong was due is a Pong missed, and ends as `Lost`,
//! whichever of the two the read loop polls first.
//!
//! The scripted clock refuses a length from the call that names it on, so a sleep armed before
//! that call keeps its script. The client answers a Ping only on a read that begins after it, so a
//! test that reads nothing leaves the Pong outstanding.

mod support;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Poll, Waker};
use std::time::{Duration, Instant};

use ulo::{BoxFuture, Module, ModuleDef, ModuleIdentity, Spawn, TaskHandle, Timer, injectable, routes};
use ulo_conformance_support::ScriptedTimer;
use ulo_runtime_tokio::Tokio;
use ulo_ws::{Connection, DisconnectReason, OnDisconnect, WsModule};

use support::{Record, Running, Socket, close_frame, hang_up, next_json, send_json, within};

const PING: Duration = Duration::from_secs(7);

/// Shorter than `PING`, so advancing past a Pong's deadline leaves the next tick pending.
const PONG: Duration = Duration::from_secs(3);

/// A scripted clock with tokio's spawning: the app's runtime.
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

/// [`Handed`] whose wait for a Pong is refused only once the Pong is due: the clock moves past
/// `PONG` as the wait is asked, as a runtime blocked past the deadline and then shut down leaves it.
/// A sleep of `PONG`, the close's wait for the client's Close, ends at once, as on that runtime.
#[derive(Clone)]
struct Late(Handed);

impl Timer for Late {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        if d == PONG {
            return Box::pin(std::future::ready(()));
        }
        self.0.sleep(d)
    }

    fn now(&self) -> Instant {
        self.0.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        if d == PONG {
            self.0.clock.advance(PONG);
            return Box::pin(std::future::ready(Err(ulo::Refused)));
        }
        self.0.try_sleep(d)
    }
}

impl Spawn for Late {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.0.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        self.0.try_spawn(fut)
    }
}

#[derive(Clone)]
struct Departures(Record<DisconnectReason>);

#[injectable]
struct Ticking {
    ends: ulo::Dep<Departures>,
}

#[routes]
#[ulo_ws::gateway(path = "/ticking", ping_interval = ulo::Bound::After(PING), pong_timeout = ulo::Bound::After(PONG))]
impl Ticking {
    #[ulo_ws::message("echo")]
    fn echo(&self) -> &'static str {
        "echoed"
    }
}

impl OnDisconnect for Ticking {
    async fn on_disconnect(&self, _conn: &Connection, why: DisconnectReason) {
        self.ends.0.push(why);
    }
}

struct Root(Departures);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(self.0.clone());
        m.controller::<Ticking>();
    }
}

struct Ticked {
    app: Running,
    clock: ScriptedTimer,
    ends: Record<DisconnectReason>,
    socket: Socket,
}

/// An app on `clock`, with one connection to `/ticking` whose first ping tick is armed or refused.
async fn connected(clock: ScriptedTimer) -> Ticked {
    let ends = Record::new();
    let runtime = Handed { clock: clock.clone(), spawn: Tokio::current() };
    let app = Running::start_on(Root(Departures(ends.clone())), ulo_http_hyper::Server::new("127.0.0.1:0"), runtime).await;
    let socket = app.connect("/ticking", &[]).await;
    until("the connection's first ping tick", || clock.live(PING) + clock.refused(PING) > 0).await;
    Ticked { app, clock, ends, socket }
}

async fn until(what: &str, done: impl Fn() -> bool) {
    within(what, async {
        while !done() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

const SHUT_DOWN: (u16, &str) = (1001, ulo_transport::SHUTDOWN_MESSAGE);

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_ping_tick_closes_the_connection_as_runtime_shut_down() {
    let clock = ScriptedTimer::new();
    clock.refuse(PING);
    let Ticked { app, clock, ends, mut socket } = connected(clock).await;
    let closed = close_frame(&mut socket).await;
    assert_eq!(closed.as_ref().map(|(code, reason)| (*code, reason.as_str())), Some(SHUT_DOWN), "the close a refused tick ends with");
    hang_up(socket).await;
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::RuntimeShutDown]);
    assert_eq!(clock.refused(PING), 1, "the ping ticks asked of a runtime refusing them");
    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ping_tick_refused_with_a_pong_outstanding_ends_as_runtime_shut_down_not_lost() {
    let Ticked { app, clock, ends, mut socket } = connected(ScriptedTimer::new()).await;
    clock.refuse(PING);
    clock.advance(PING);
    // The first tick's Ping went out, and the wait for its Pong and the refused second tick were
    // armed together.
    until("the second ping tick", || clock.refused(PING) > 0).await;
    let closed = close_frame(&mut socket).await;
    assert_eq!(closed.as_ref().map(|(code, reason)| (*code, reason.as_str())), Some(SHUT_DOWN), "the close a refused tick ends with");
    hang_up(socket).await;
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::RuntimeShutDown]);
    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_wait_for_a_pong_ends_the_connection_as_runtime_shut_down_not_lost() {
    let clock = ScriptedTimer::new();
    clock.refuse(PONG);
    let Ticked { app, clock, ends, mut socket } = connected(clock).await;
    clock.advance(PING);
    // The close's wait for the client's Close is `PONG` long, refused too, so the connection ends
    // without it.
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::RuntimeShutDown]);
    let closed = close_frame(&mut socket).await;
    assert_eq!(closed.as_ref().map(|(code, reason)| (*code, reason.as_str())), Some(SHUT_DOWN), "the close a refused Pong wait ends with");
    drop(socket);
    app.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pong_that_misses_its_timeout_on_a_live_clock_ends_as_lost() {
    let Ticked { app, clock, ends, socket } = connected(ScriptedTimer::new()).await;
    clock.advance(PING);
    until("the wait for the first Pong", || clock.live(PONG) == 1).await;
    clock.advance(PONG);
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::Lost]);
    drop(socket);
    app.stop().await;
}

/// A refusal of the wait for a Pong that arrives once the Pong was due is a Pong missed: the peer
/// was late, and the connection ends as `Lost`.
#[tokio::test(flavor = "multi_thread")]
async fn a_pong_wait_refused_after_the_pong_was_due_ends_as_lost() {
    let clock = ScriptedTimer::new();
    let ends = Record::new();
    let runtime = Late(Handed { clock: clock.clone(), spawn: Tokio::current() });
    let app = Running::start_on(Root(Departures(ends.clone())), ulo_http_hyper::Server::new("127.0.0.1:0"), runtime).await;
    let socket = app.connect("/ticking", &[]).await;
    until("the connection's first ping tick", || clock.live(PING) > 0).await;
    clock.advance(PING);
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::Lost]);
    drop(socket);
    app.stop().await;
}

/// Waits that stay pending until [`open`](Self::open), then answer `Err(Refused)`: a runtime blocked
/// past a deadline and then shut down, whose timer never fired for the waits armed on it.
#[derive(Clone, Default)]
struct Gate(Arc<GateState>);

#[derive(Default)]
struct GateState {
    open: AtomicBool,
    waiting: Mutex<Vec<Waker>>,
    armed: AtomicUsize,
}

impl Gate {
    fn wait(&self) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        self.0.armed.fetch_add(1, Ordering::SeqCst);
        let gate = Arc::clone(&self.0);
        Box::pin(std::future::poll_fn(move |cx| {
            if !gate.open.load(Ordering::SeqCst) {
                gate.waiting.lock().unwrap().push(cx.waker().clone());
                if !gate.open.load(Ordering::SeqCst) {
                    return Poll::Pending;
                }
            }
            Poll::Ready(Err(ulo::Refused))
        }))
    }

    fn armed(&self) -> usize {
        self.0.armed.load(Ordering::SeqCst)
    }

    fn open(&self) {
        self.0.open.store(true, Ordering::SeqCst);
        let waiting = std::mem::take(&mut *self.0.waiting.lock().unwrap());
        waiting.into_iter().for_each(Waker::wake);
    }
}

/// [`Handed`] whose first ping tick is scripted, and whose later ticks, with `pong` set the wait
/// for a Pong as well, wait on two gates: `ping` and `pong`. A sleep of `PONG`, the close's wait for
/// the client's Close, ends at once, as on a runtime that has shut down.
#[derive(Clone)]
struct Blocked {
    handed: Handed,
    ticks: Arc<AtomicUsize>,
    ping: Gate,
    pong: Gate,
}

impl Blocked {
    fn new(clock: ScriptedTimer) -> Blocked {
        Blocked { handed: Handed { clock, spawn: Tokio::current() }, ticks: Arc::default(), ping: Gate::default(), pong: Gate::default() }
    }
}

impl Timer for Blocked {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        if d == PONG {
            return Box::pin(std::future::ready(()));
        }
        self.handed.sleep(d)
    }

    fn now(&self) -> Instant {
        self.handed.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        if d == PONG {
            return self.pong.wait();
        }
        if d == PING && self.ticks.fetch_add(1, Ordering::SeqCst) > 0 {
            return self.ping.wait();
        }
        self.handed.try_sleep(d)
    }
}

impl Spawn for Blocked {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.handed.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        self.handed.try_spawn(fut)
    }
}

/// A connection on `runtime` whose first Ping went out after `echoes` echoed messages, with the
/// Pong now due on the clock and both waits armed on their gates.
async fn overdue(runtime: Blocked, echoes: usize) -> (Running, Record<DisconnectReason>, Socket) {
    let clock = runtime.handed.clock.clone();
    let (ping, pong) = (runtime.ping.clone(), runtime.pong.clone());
    let ends = Record::new();
    let app = Running::start_on(Root(Departures(ends.clone())), ulo_http_hyper::Server::new("127.0.0.1:0"), runtime).await;
    let mut socket = app.connect("/ticking", &[]).await;
    for id in 0..echoes {
        send_json(&mut socket, &serde_json::json!({ "event": "echo", "id": id })).await;
        next_json(&mut socket).await;
    }
    until("the connection's first ping tick", || clock.live(PING) == 1).await;
    clock.advance(PING);
    until("the second tick and the wait for the first Pong", || ping.armed() == 1 && pong.armed() == 1).await;
    clock.advance(PONG);
    (app, ends, socket)
}

/// A ping tick refused once the outstanding Pong was due is a Pong missed: the connection ends as
/// `Lost`, though the wait for the Pong has not answered.
#[tokio::test(flavor = "multi_thread")]
async fn a_ping_tick_refused_once_the_pong_was_due_ends_as_lost() {
    let runtime = Blocked::new(ScriptedTimer::new());
    let ping = runtime.ping.clone();
    let (app, ends, socket) = overdue(runtime, 0).await;
    ping.open();
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::Lost]);
    drop(socket);
    app.stop().await;
}

/// Both waits refused in one turn once the Pong was due end as `Lost` whichever the read loop polls
/// first: the echoes before the Ping move the turn the refusals arrive in through every branch.
#[tokio::test(flavor = "multi_thread")]
async fn both_waits_refused_once_the_pong_was_due_end_as_lost_in_any_poll_order() {
    for echoes in 0..9 {
        let runtime = Blocked::new(ScriptedTimer::new());
        let (ping, pong) = (runtime.ping.clone(), runtime.pong.clone());
        let (app, ends, socket) = overdue(runtime, echoes).await;
        ping.open();
        pong.open();
        assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::Lost], "after {echoes} echoes");
        drop(socket);
        app.stop().await;
    }
}

/// The connection a gateway's `OnConnect` handed over, for [`Closer`] to close.
#[derive(Clone, Default)]
struct Opened(std::sync::Arc<std::sync::Mutex<Option<Connection>>>);

#[injectable]
struct Closing {
    ends: ulo::Dep<Departures>,
    opened: ulo::Dep<Opened>,
}

#[routes]
#[ulo_ws::gateway(path = "/closing", ping_interval = ulo::Bound::After(PING), pong_timeout = ulo::Bound::After(PONG))]
impl Closing {
    #[ulo_ws::message("echo")]
    fn echo(&self) -> &'static str {
        "echoed"
    }
}

impl ulo_ws::OnConnect for Closing {
    async fn on_connect(&self, cx: &ulo_ws::ConnectCx) -> Result<(), ulo_ws::ConnectRefused> {
        *self.opened.0.lock().unwrap() = Some(cx.conn().clone());
        Ok(())
    }
}

impl OnDisconnect for Closing {
    async fn on_disconnect(&self, _conn: &Connection, why: DisconnectReason) {
        self.ends.0.push(why);
    }
}

struct ClosingRoot(Departures, Opened);

impl Module for ClosingRoot {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.import(WsModule::for_root());
        m.value(self.0.clone());
        m.value(self.1.clone());
        m.controller::<Closing>();
    }
}

/// [`Handed`] whose first ping tick, polled, queues the gateway's own close and is then refused:
/// the close and the refusal reach the read loop in one turn.
#[derive(Clone)]
struct Closer(Handed, Opened);

impl Timer for Closer {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        self.0.sleep(d)
    }

    fn now(&self) -> Instant {
        self.0.now()
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        let opened = self.1.0.lock().unwrap().clone();
        match opened {
            Some(conn) if d == PING => Box::pin(async move {
                conn.close(4000, "bye").await;
                Err(ulo::Refused)
            }),
            _ => self.0.try_sleep(d),
        }
    }
}

impl Spawn for Closer {
    fn spawn(&self, fut: BoxFuture<'static, ()>) -> TaskHandle {
        self.0.spawn(fut)
    }

    fn try_spawn(&self, fut: BoxFuture<'static, ()>) -> Result<TaskHandle, ulo::Refused> {
        self.0.try_spawn(fut)
    }
}

/// R2: a close the gateway queued outranks a refusal that reaches the read loop in the same turn,
/// so the client reads the gateway's code and the connection ends `ServerClose`.
#[tokio::test(flavor = "multi_thread")]
async fn a_gateway_close_queued_with_a_refusal_outranks_it() {
    let ends = Record::new();
    let opened = Opened::default();
    let runtime = Closer(Handed { clock: ScriptedTimer::new(), spawn: Tokio::current() }, opened.clone());
    let app = Running::start_on(ClosingRoot(Departures(ends.clone()), opened), ulo_http_hyper::Server::new("127.0.0.1:0"), runtime).await;
    let mut socket = app.connect("/closing", &[]).await;
    let closed = close_frame(&mut socket).await;
    assert_eq!(closed.as_ref().map(|(code, reason)| (*code, reason.as_str())), Some((4000, "bye")), "the close a gateway queued beside a refusal");
    hang_up(socket).await;
    assert_eq!(ends.at_least(1, "`on_disconnect`").await, vec![DisconnectReason::ServerClose { code: 4000 }]);
    app.stop().await;
}
