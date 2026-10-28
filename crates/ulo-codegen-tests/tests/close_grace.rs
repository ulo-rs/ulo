//! The gRPC server's `close` gives the connections 500 ms to end once the drain has begun, timed
//! by the app's clock. The app here runs on a clock that holds every sleep until the test fires
//! it, and a client whose 64 KiB HTTP/2 windows its reply stream fills, and which reads nothing,
//! holds its connection open past the drain: the server's send waits for a `WINDOW_UPDATE` that
//! never comes. `close` asks the clock for the grace once the drain window has ended, and returns
//! once the test fires it.

mod support;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::{Notify, oneshot, watch};
use ulo::{App, BoxFuture, Dep, Module, ModuleDef, ModuleIdentity, Signal, Timer, injectable, routes};
use ulo_codegen_tests::pb::counter_client::CounterClient;
use ulo_codegen_tests::pb::{self, Number};
use ulo_grpc::Message as Grpc;
use ulo_transport::CallError;

use support::within;

/// The drain window, a length nothing else in the app asks the clock for.
const WINDOW: Duration = Duration::from_secs(7);

/// What `close` gives the connections after the drain window: the server's `CLOSE_GRACE`.
const GRACE: Duration = Duration::from_millis(500);

#[derive(Clone, Default)]
struct Entered(Arc<Notify>);

/// `Counter.Count` as a flood: `up_to` numbers, every one ready, each a ten-byte varint.
#[injectable]
struct Flood {
    entered: Dep<Entered>,
}

#[routes]
impl Flood {
    #[ulo_grpc::method(pb::counter::Count)]
    async fn count(&self, req: Grpc<pb::CountRequest>) -> impl futures_util::Stream<Item = Result<Number, CallError>> {
        self.entered.0.notify_one();
        futures_util::stream::iter((0..i64::from(req.0.up_to)).map(|n| Ok(Number { value: i64::MAX - n })))
    }
}

struct Root(Entered);

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        m.value(self.0.clone());
        m.controller::<Flood>();
    }
}

/// The app's clock: every sleep is held until the test fires it, and each one asked for is
/// recorded by its length. `now` stands still, so a wait computed from a deadline asks for the
/// length it was configured with.
#[derive(Clone)]
struct Held(Arc<Sleeps>);

struct Sleeps {
    base: Instant,
    asked: Mutex<Vec<(Duration, Option<oneshot::Sender<()>>)>>,
    changed: watch::Sender<usize>,
}

impl Timer for Held {
    fn sleep(&self, d: Duration) -> BoxFuture<'static, ()> {
        let (fire, fired) = oneshot::channel::<()>();
        lock(&self.0.asked).push((d, Some(fire)));
        self.0.changed.send_modify(|asked| *asked += 1);
        Box::pin(async move {
            let _ = fired.await;
        })
    }

    fn now(&self) -> Instant {
        self.0.base
    }

    fn try_sleep(&self, d: Duration) -> BoxFuture<'static, Result<(), ulo::Refused>> {
        let sleep = Timer::sleep(self, d);
        Box::pin(async move {
            sleep.await;
            Ok(())
        })
    }
}

impl Held {
    fn new() -> Held {
        Held(Arc::new(Sleeps { base: Instant::now(), asked: Mutex::new(Vec::new()), changed: watch::Sender::new(0) }))
    }

    /// How many sleeps of `d` have been asked for.
    fn asked(&self, d: Duration) -> usize {
        lock(&self.0.asked).iter().filter(|(asked, _)| *asked == d).count()
    }

    /// Waits until a sleep of `d` has been asked for.
    async fn asked_for(&self, d: Duration, what: &str) {
        let mut changed = self.0.changed.subscribe();
        let asked = within(what, changed.wait_for(|_| self.asked(d) > 0)).await;
        asked.unwrap_or_else(|_| panic!("the clock's record closed while waiting for {what}"));
    }

    /// Ends every sleep of `d` asked for so far.
    fn fire(&self, d: Duration) {
        for (asked, fire) in lock(&self.0.asked).iter_mut() {
            if *asked == d
                && let Some(fire) = fire.take()
            {
                let _ = fire.send(());
            }
        }
    }

    fn lengths(&self) -> Vec<Duration> {
        lock(&self.0.asked).iter().map(|(asked, _)| *asked).collect()
    }
}

// One thread: a `close` that would end in the poll that asks for its grace has ended before the
// test reads it.
#[tokio::test(flavor = "current_thread")]
async fn close_waits_out_its_grace_on_the_apps_clock_for_a_connection_that_outlives_the_drain() {
    let (entered, clock) = (Entered::default(), Held::new());
    let app = App::builder(Root(entered.clone()))
        .runtime(ulo_runtime_tokio::Tokio::current())
        .timer(clock.clone())
        .drain_timeout(WINDOW)
        .wire()
        .unwrap_or_else(|error| panic!("the app did not wire: {error}"))
        .connect()
        .await
        .unwrap_or_else(|error| panic!("the app did not connect: {error}"))
        .bind(ulo_grpc::Server::new("127.0.0.1:0").reflection(false))
        .listen()
        .await
        .unwrap_or_else(|error| panic!("the app did not listen: {error}"));
    let addr = app.addresses().first().expect("one address").addr;
    let handle = app.handle();
    let serving = tokio::spawn(async move { app.serve(std::future::pending::<Signal>()).await });
    let channel = tonic::transport::Endpoint::from_shared(format!("http://{addr}"))
        .expect("an endpoint")
        .initial_stream_window_size(65_535)
        .initial_connection_window_size(65_535)
        .connect()
        .await
        .expect("the gRPC channel");
    let reply = CounterClient::new(channel.clone()).count(pb::CountRequest { up_to: 400_000 }).await.expect("the stream's head");
    within("the handler entering", entered.0.notified()).await;

    let closing = {
        let handle = handle.clone();
        tokio::spawn(async move { handle.close(Signal::new("grace")).await })
    };
    clock.asked_for(WINDOW, "the drain window").await;
    assert_eq!(clock.asked(GRACE), 0, "the grace was asked for before the drain window ended: {:?}", clock.lengths());
    clock.fire(WINDOW);
    clock.asked_for(GRACE, "the close grace").await;
    assert!(!closing.is_finished(), "close returned before its grace ended, with the connection still open");

    clock.fire(GRACE);
    let closed = within("close, once its grace ended", closing).await.expect("the close task");
    let shutdown = closed.unwrap_or_else(|error| panic!("the close failed: {error}"));
    assert_eq!(shutdown.abandoned, 1, "the flooding call, live at the drain window's end");
    assert_eq!(clock.asked(GRACE), 1, "one grace per close: {:?}", clock.lengths());
    let served = within("serve", serving).await.expect("the serving task");
    let served = served.unwrap_or_else(|error| panic!("serve answered the shutdown's failure: {error}"));
    assert_eq!(served.signal, Signal::new("grace"), "serve answers the signal that closed the app");
    drop((channel, reply));
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
