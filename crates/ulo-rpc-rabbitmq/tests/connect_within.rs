//! `connect_within` bounds a server's `listen` and a client's connect. Against a socket that
//! accepts and never answers the AMQP handshake, `listen` fails as `Bind` carrying
//! `ConnectTimedOut` once the bound has passed, and a call fails `Unavailable` naming it. A zero
//! bound is refused before any I/O, and a clock that refuses fails `listen` as `Permanent`
//! `RuntimeShutDown`.
//!
//! A client's connect belongs to the link, not to the call that started it: a failed connect is
//! replaced by the next call's, and `close` or the link's drop ends one still running, `close`
//! failing the `connect` waiting on it.
//!
//! The bound runs on the app's `Timer`, which these tests script: no bound elapses in real time.
//! Each connect that runs out leaves lapin's `lapin-io-loop` thread and its socket behind
//! (transports DESIGN §5.2), so the silent socket's connections stay open until the test process
//! ends; `tests/one_connect.rs` counts the threads.

use std::error::Error;
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ulo::testing::ScriptedTimer;
use ulo::{App, Module, ModuleDef, ModuleIdentity, RuntimeShutDown, Signal, StartupError, is_permanent};
use ulo_rpc::{ErrorKind, Link, RpcClient, Server, UnusableLink};
use ulo_rpc_rabbitmq::{ConnectTimedOut, RabbitMq};
use ulo_runtime_tokio::Tokio;

struct Root;

impl Module for Root {
    fn identity(&self) -> ModuleIdentity {
        ModuleIdentity::of_type::<Self>()
    }

    fn register(&self, m: &mut ModuleDef<'_>) {
        let _ = m;
    }
}

/// The link's bound in these tests; the scripted clock moves past it, the wall clock does not. It
/// differs from the client's 5 s call timeout, so a client's two waits are told apart.
const WITHIN: Duration = Duration::from_secs(3);

/// How long the tests wait, in real time, for the link to reach a state they then act on, and for
/// an answer once the clock has moved.
const PATIENCE: Duration = Duration::from_secs(10);

/// A loopback socket that accepts every connection and never writes to one.
struct Silent {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
}

impl Silent {
    fn start() -> Silent {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let addr = listener.local_addr().expect("the port's address");
        let accepted = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&accepted);
        // Holds every accepted socket, so none closes; the thread ends with the process.
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming().flatten() {
                held.push(stream);
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        Silent { addr, accepted }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    /// A url naming the socket, with credentials the error must not repeat.
    fn url(&self) -> String {
        format!("amqp://guest:hunter2@{}/%2f", self.addr)
    }
}

async fn app(clock: &ScriptedTimer) -> App<ulo::Connected> {
    App::builder(Root).runtime(Tokio::current()).timer(clock.clone()).wire().expect("the app wires").connect().await.expect("the app connects")
}

/// `listen()` over `link` on an app timed by `clock`; a server that listens is closed and fails
/// the test.
#[allow(clippy::result_large_err, reason = "`StartupError` is the error `listen` returns, handed through whole")]
async fn listen_on(link: RabbitMq, clock: ScriptedTimer) -> Result<(), StartupError> {
    let bound = app(&clock).await.bind(Server::new(link)).listen().await?;
    let _ = bound.handle().close(Signal::new("listened")).await;
    panic!("a server listened against a broker that never answers");
}

/// The error `listen()` over `link` fails with, failing the test naming `what` when it has not
/// returned after [`PATIENCE`].
async fn failed_listen(link: RabbitMq, clock: ScriptedTimer, what: &str) -> StartupError {
    match tokio::time::timeout(PATIENCE, listen_on(link, clock)).await {
        Ok(listened) => listened.expect_err("`listen_on` fails the test when a server listens"),
        Err(_) => panic!("{what} did not return within {PATIENCE:?}"),
    }
}

/// Waits, in real time, until `done` holds, failing naming `what` after [`PATIENCE`].
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// `error` and each `source()` below it.
fn chain<'e>(error: &'e (dyn Error + 'static)) -> impl Iterator<Item = &'e (dyn Error + 'static)> {
    let mut next = Some(error);
    std::iter::from_fn(move || {
        let current = next?;
        next = current.source();
        Some(current)
    })
}

fn bind_cause(error: StartupError) -> ulo::BoxError {
    match error {
        StartupError::Bind { source, .. } => source.into_inner(),
        other => panic!("expected `Bind`, got: {other}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listen_against_a_silent_broker_fails_as_bind_at_the_bound() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let listening = tokio::spawn(listen_on(RabbitMq::url(silent.url()).connect_within(WITHIN), clock.clone()));
    until("the bound armed and the first connection accepted", || clock.live(WITHIN) == 1 && silent.accepted() == 1).await;
    assert!(!listening.is_finished(), "listen returned before the app's clock reached the bound");
    clock.advance(WITHIN);
    let error = tokio::time::timeout(PATIENCE, listening)
        .await
        .unwrap_or_else(|_| panic!("listen did not return within {PATIENCE:?} of the bound passing"))
        .expect("listen did not panic")
        .expect_err("listen succeeded against a broker that never answers");
    assert!(!is_permanent(&error), "a silent broker is marked permanent, though a later start may reach it: {error}");
    let text = error.to_string();
    let cause = bind_cause(error);
    let timed_out = cause.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("the `Bind` does not carry `ConnectTimedOut`: {cause:?}"));
    assert_eq!(timed_out.within(), WITHIN);
    assert_eq!(timed_out.address(), silent.addr.to_string());
    assert_eq!(timed_out.link(), "RabbitMQ", "the expiry names another link");
    let shown = cause.to_string();
    assert!(shown.contains(&silent.addr.to_string()) && shown.contains(&format!("{WITHIN:?}")), "the cause names the address and the bound: {shown}");
    assert!(!format!("{text} {shown}").contains("hunter2"), "the failure repeats the url's password: {text} / {shown}");
    assert_eq!(silent.accepted(), 1, "the link connected again before the bound passed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_against_a_silent_broker_fails_its_call_at_the_bound() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let client = RpcClient::new(RabbitMq::url(silent.url()).connect_within(WITHIN), runtime).expect("the link is usable");
    let calling = tokio::spawn(async move { client.request::<_, u32>("silent.call", &()).await });
    until("the connect bound armed and the connection accepted", || clock.live(WITHIN) == 1 && silent.accepted() == 1).await;
    assert!(!calling.is_finished(), "the call ended before the app's clock reached the connect bound");
    clock.advance(WITHIN);
    let failed = tokio::time::timeout(PATIENCE, calling)
        .await
        .unwrap_or_else(|_| panic!("the call did not end within {PATIENCE:?} of the bound passing"))
        .expect("the call did not panic")
        .expect_err("a call succeeded against a broker that never answers");
    assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, Some("connect_timed_out")), "{failed:?}");
    let message = failed.message();
    assert!(message.contains(&silent.addr.to_string()) && message.contains(&format!("{WITHIN:?}")), "the call names the address and the bound: {message}");
    assert!(message.contains("connect_within"), "the call names the setting that ran out: {message}");
    assert!(message.contains("the RabbitMQ broker at"), "the call's expiry names another link: {message}");
    assert!(!message.contains("hunter2"), "the call repeats the url's password: {message}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_connect_within_is_refused_at_prepare_and_where_a_client_takes_the_link() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let refused = failed_listen(RabbitMq::url(silent.url()).connect_within(Duration::ZERO), clock, "listen with a zero bound").await;
    let StartupError::Configure(_) = &refused else { panic!("expected a `Configure` refusal, got: {refused}") };
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");

    let built = RpcClient::new(RabbitMq::url(silent.url()).connect_within(Duration::ZERO), Arc::new(Tokio::current()));
    let refused: UnusableLink = match built {
        Ok(_) => panic!("a client was built over a zero `connect_within`"),
        Err(refused) => refused,
    };
    assert_eq!(refused.link(), "rabbitmq");
    let cause = refused.source().expect("the refusal carries its cause");
    assert!(cause.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {cause}");
    assert_eq!(silent.accepted(), 0, "a zero bound reached the network");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_clock_fails_listen_as_a_permanent_runtime_shut_down() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    clock.refuse(WITHIN);
    let error = failed_listen(RabbitMq::url(silent.url()).connect_within(WITHIN), clock.clone(), "listen on a refusing clock").await;
    assert_eq!(clock.refused(WITHIN), 1, "the bound was asked of the clock other than once");
    assert!(is_permanent(&error), "a clock whose runtime has shut down cannot time a retry: {error}");
    let cause = bind_cause(error);
    assert!(chain(&*cause).any(|error| error.is::<RuntimeShutDown>()), "the `Bind` does not carry `RuntimeShutDown`: {cause:?}");
    assert!(!chain(&*cause).any(|error| error.is::<ConnectTimedOut>()), "a refused bound is reported as its expiry: {cause:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_clock_fails_a_client_s_call_as_runtime_shut_down() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    clock.refuse(WITHIN);
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let client = RpcClient::new(RabbitMq::url(silent.url()).connect_within(WITHIN), runtime).expect("the link is usable");
    let failed = tokio::time::timeout(PATIENCE, client.request::<_, u32>("silent.call", &()))
        .await
        .unwrap_or_else(|_| panic!("the call on a refusing clock did not end within {PATIENCE:?}"))
        .expect_err("a call succeeded on a clock that refuses its connect bound");
    assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, Some(ulo::RUNTIME_SHUT_DOWN)), "{failed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unset_bound_is_thirty_seconds() {
    const UNSET: Duration = Duration::from_secs(30);
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let listening = tokio::spawn(listen_on(RabbitMq::url(silent.url()), clock.clone()));
    until("the unset bound armed and the first connection accepted", || clock.live(UNSET) == 1 && silent.accepted() == 1).await;
    clock.advance(UNSET);
    let error = tokio::time::timeout(PATIENCE, listening)
        .await
        .unwrap_or_else(|_| panic!("listen did not return within {PATIENCE:?} of the unset bound passing"))
        .expect("listen did not panic")
        .expect_err("listen succeeded against a broker that never answers");
    let cause = bind_cause(error);
    let timed_out = cause.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("the `Bind` does not carry `ConnectTimedOut`: {cause:?}"));
    assert_eq!(timed_out.within(), UNSET);
}

/// A loopback socket that accepts every connection and closes it at once, counting each.
fn closing() -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the port's address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&accepted);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            drop(stream);
        }
    });
    (addr, accepted)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_connect_is_replaced_by_the_next_call_s() {
    let (addr, accepted) = closing();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let client = RpcClient::new(RabbitMq::url(format!("amqp://guest:guest@{addr}/%2f")).connect_within(WITHIN), runtime).expect("the link is usable");
    for attempt in 1..=2 {
        let failed = tokio::time::timeout(PATIENCE, client.request::<_, u32>("closing.call", &()))
            .await
            .unwrap_or_else(|_| panic!("call {attempt} against a broker that closes the connection did not end within {PATIENCE:?}"))
            .expect_err("a call succeeded against a broker that closes the connection");
        assert_eq!(failed.kind(), ErrorKind::Unavailable, "call {attempt}: {failed:?}");
        assert!(!failed.message().contains("connect_within"), "call {attempt} failed at the bound rather than on the closed connection: {failed:?}");
        assert_eq!(accepted.load(Ordering::SeqCst), attempt, "call {attempt} did not connect once of its own");
        assert_eq!(clock.live(WITHIN), 0, "the failed connect's bound is still armed after call {attempt}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn close_fails_the_connect_it_cuts_short() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let link = Arc::new(RabbitMq::url(silent.url()).connect_within(WITHIN));
    let connecting = {
        let link = Arc::clone(&link);
        tokio::spawn(async move { link.connect(&runtime).await.map(drop) })
    };
    until("the connect bound armed and the connection accepted", || clock.live(WITHIN) == 1 && silent.accepted() == 1).await;
    link.close().await.expect("the link closes");
    let failed = tokio::time::timeout(PATIENCE, connecting)
        .await
        .unwrap_or_else(|_| panic!("the connect `close` cut short did not end within {PATIENCE:?}"))
        .expect("the connect did not panic")
        .expect_err("a connect succeeded against a broker that never answers");
    assert!(failed.to_string().contains("closed"), "the connect does not answer that the link closed: {failed}");
    until("the closed link's connect bound dropped", || clock.live(WITHIN) == 0).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_dropped_link_aborts_its_connect() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let link = RabbitMq::url(silent.url()).connect_within(WITHIN);
    // The call gives up at once; the connect it started runs on until the link drops.
    assert!(futures_util::FutureExt::now_or_never(link.connect(&runtime)).is_none(), "a connect against a silent broker answered");
    until("the connect bound armed and the connection accepted", || clock.live(WITHIN) == 1 && silent.accepted() == 1).await;
    drop(link);
    until("the dropped link's connect bound dropped", || clock.live(WITHIN) == 0).await;
}

/// A loopback socket that accepts every connection, counting each, holds it unanswered until
/// the returned gate opens, then closes it.
fn held() -> (SocketAddr, Arc<AtomicUsize>, Arc<(Mutex<bool>, Condvar)>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the port's address");
    let (accepted, gate) = (Arc::new(AtomicUsize::new(0)), Arc::new((Mutex::new(false), Condvar::new())));
    let (counted, waited) = (Arc::clone(&accepted), Arc::clone(&gate));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            counted.fetch_add(1, Ordering::SeqCst);
            let waited = Arc::clone(&waited);
            std::thread::spawn(move || {
                let (open, changed) = &*waited;
                let guard = open.lock().unwrap_or_else(PoisonError::into_inner);
                drop(changed.wait_while(guard, |open| !*open).unwrap_or_else(PoisonError::into_inner));
                drop(stream);
            });
        }
    });
    (addr, accepted, gate)
}

/// Two `connect`s waiting on one connect that fails both answer its failure, and the link
/// connected once: a failure stays for every `connect` that waited on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connects_waiting_on_one_failed_connect_share_its_failure() {
    let (addr, accepted, gate) = held();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let link = Arc::new(RabbitMq::url(format!("amqp://guest:guest@{addr}/%2f")).connect_within(WITHIN));
    let both = {
        let link = Arc::clone(&link);
        tokio::spawn(async move {
            let (first, second) = futures_util::future::join(link.connect(&runtime), link.connect(&runtime)).await;
            (first.map(drop), second.map(drop))
        })
    };
    until("the shared connect accepted", || accepted.load(Ordering::SeqCst) == 1).await;
    {
        let (open, changed) = &*gate;
        *open.lock().unwrap_or_else(PoisonError::into_inner) = true;
        changed.notify_all();
    }
    let (first, second) = tokio::time::timeout(PATIENCE, both)
        .await
        .unwrap_or_else(|_| panic!("the two connects did not end within {PATIENCE:?}"))
        .expect("the connects did not panic");
    let (first, second) = (first.expect_err("a connect succeeded against a closed connection"), second.expect_err("a connect succeeded against a closed connection"));
    assert_eq!(first.to_string(), second.to_string(), "the two connects answered different failures");
    assert_eq!(accepted.load(Ordering::SeqCst), 1, "a connect waiting on a failed connect connected again rather than answering its failure");
}
