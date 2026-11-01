//! `connect_within` bounds a server's `listen` and a client's connect. Against a socket that
//! accepts and never writes the server's `INFO`, `listen` fails as `Bind` carrying
//! `ConnectTimedOut` once the bound has passed, and a call fails `Unavailable` with
//! `reason: "connect_timed_out"`; either way the one connection the link made is closed. A zero
//! bound is refused before any I/O, and a clock that refuses fails `listen` as `Permanent`
//! `RuntimeShutDown` and a call `Unavailable` with `reason: "runtime_shut_down"`. With both unset,
//! a call's own 5 s timeout ends the connect first, and closes its connection too.
//!
//! The bound runs on the app's `Timer`, which these tests script: no bound elapses in real time.
//! A server's `listen` first waits for the platform's root certificates, under a `roots_timeout`
//! the scripted clock never reaches; the load itself takes real time, which [`PATIENCE`] allows
//! for.

use std::error::Error;
use std::io::Read;
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ulo::testing::ScriptedTimer;
use ulo::{App, Module, ModuleDef, ModuleIdentity, RuntimeShutDown, Signal, StartupError, is_permanent};
use ulo_rpc::{ConnectTimedOut, ErrorKind, RpcClient, Server, UnusableLink};
use ulo_rpc_nats::Nats;
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
const PATIENCE: Duration = Duration::from_secs(20);

/// A loopback socket that accepts every connection, never writes to one, and counts the
/// connections accepted and the ones their peer has closed.
struct Silent {
    addr: SocketAddr,
    accepted: Arc<AtomicUsize>,
    closed: Arc<AtomicUsize>,
}

impl Silent {
    fn start() -> Silent {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port");
        let addr = listener.local_addr().expect("the port's address");
        let (accepted, closed) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let (counted, ended) = (Arc::clone(&accepted), Arc::clone(&closed));
        // The threads end with the process.
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                counted.fetch_add(1, Ordering::SeqCst);
                let ended = Arc::clone(&ended);
                // A NATS client writes nothing before the server's `INFO`, so the read returns
                // only at the peer's close, or fails at its reset.
                std::thread::spawn(move || {
                    let mut buf = [0u8; 256];
                    while let Ok(1..) = stream.read(&mut buf) {}
                    ended.fetch_add(1, Ordering::SeqCst);
                });
            }
        });
        Silent { addr, accepted, closed }
    }

    fn accepted(&self) -> usize {
        self.accepted.load(Ordering::SeqCst)
    }

    fn closed(&self) -> usize {
        self.closed.load(Ordering::SeqCst)
    }

    /// A url naming the socket, with credentials the error must not repeat.
    fn url(&self) -> String {
        format!("nats://ulo:hunter2@{}", self.addr)
    }
}

async fn app(clock: &ScriptedTimer) -> App<ulo::Connected> {
    App::builder(Root).runtime(Tokio::current()).timer(clock.clone()).wire().expect("the app wires").connect().await.expect("the app connects")
}

/// `listen()` over `link` on an app timed by `clock`; a server that listens is closed and fails
/// the test.
#[allow(clippy::result_large_err, reason = "`StartupError` is the error `listen` returns, handed through whole")]
async fn listen_on(link: Nats, clock: ScriptedTimer) -> Result<(), StartupError> {
    let bound = app(&clock).await.bind(Server::new(link.group("connect_within"))).listen().await?;
    let _ = bound.handle().close(Signal::new("listened")).await;
    panic!("a server listened against a NATS server that never answers");
}

/// The error `listen()` over `link` fails with, failing the test naming `what` when it has not
/// returned after [`PATIENCE`].
async fn failed_listen(link: Nats, clock: ScriptedTimer, what: &str) -> StartupError {
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
async fn listen_against_a_silent_server_fails_as_bind_at_the_bound_and_closes_its_connection() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let listening = tokio::spawn(listen_on(Nats::url(silent.url()).connect_within(WITHIN), clock.clone()));
    until("the bound armed and the connection accepted", || clock.live(WITHIN) == 1 && silent.accepted() == 1).await;
    assert!(!listening.is_finished(), "listen returned before the app's clock reached the bound");
    assert_eq!(silent.closed(), 0, "the connection closed before the bound passed");
    clock.advance(WITHIN);
    let error = tokio::time::timeout(PATIENCE, listening)
        .await
        .unwrap_or_else(|_| panic!("listen did not return within {PATIENCE:?} of the bound passing"))
        .expect("listen did not panic")
        .expect_err("listen succeeded against a NATS server that never answers");
    assert!(!is_permanent(&error), "a silent server is marked permanent, though a later start may reach it: {error}");
    let text = error.to_string();
    let cause = bind_cause(error);
    let timed_out = cause.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("the `Bind` does not carry `ConnectTimedOut`: {cause:?}"));
    assert_eq!((timed_out.within(), timed_out.address(), timed_out.link()), (WITHIN, silent.addr.to_string().as_str(), "NATS"));
    let shown = cause.to_string();
    assert!(shown.contains(&silent.addr.to_string()) && shown.contains(&format!("{WITHIN:?}")), "the cause names the address and the bound: {shown}");
    assert!(!format!("{text} {shown}").contains("hunter2"), "the failure repeats the url's password: {text} / {shown}");
    until("the connection closed after the bound", || silent.closed() == 1).await;
    assert_eq!(silent.accepted(), 1, "the link connected again before or after the bound passed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_against_a_silent_server_fails_its_call_at_the_bound_and_closes_its_connection() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let client = RpcClient::new(Nats::url(silent.url()).connect_within(WITHIN), runtime).expect("the link is usable");
    let calling = tokio::spawn(async move { client.request::<_, u32>("silent.call", &()).await });
    until("the connect bound armed and the connection accepted", || clock.live(WITHIN) == 1 && silent.accepted() == 1).await;
    assert!(!calling.is_finished(), "the call ended before the app's clock reached the connect bound");
    clock.advance(WITHIN);
    let failed = tokio::time::timeout(PATIENCE, calling)
        .await
        .unwrap_or_else(|_| panic!("the call did not end within {PATIENCE:?} of the bound passing"))
        .expect("the call did not panic")
        .expect_err("a call succeeded against a NATS server that never answers");
    assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, Some("connect_timed_out")), "{failed:?}");
    let message = failed.message();
    assert!(message.contains(&silent.addr.to_string()) && message.contains(&format!("{WITHIN:?}")), "the call names the address and the bound: {message}");
    assert!(message.contains("connect_within"), "the call names the setting that ran out: {message}");
    assert!(!message.contains("hunter2"), "the call repeats the url's password: {message}");
    until("the connection closed after the bound", || silent.closed() == 1).await;
    assert_eq!(silent.accepted(), 1, "the link connected again before or after the bound passed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_zero_connect_within_is_refused_at_prepare_and_where_a_client_takes_the_link() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let refused = failed_listen(Nats::url(silent.url()).connect_within(Duration::ZERO), clock, "listen with a zero bound").await;
    let StartupError::Configure(_) = &refused else { panic!("expected a `Configure` refusal, got: {refused}") };
    assert!(refused.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {refused}");

    let built = RpcClient::new(Nats::url(silent.url()).connect_within(Duration::ZERO), Arc::new(Tokio::current()));
    let refused: UnusableLink = match built {
        Ok(_) => panic!("a client was built over a zero `connect_within`"),
        Err(refused) => refused,
    };
    assert_eq!(refused.link(), "nats");
    let cause = refused.source().expect("the refusal carries its cause");
    assert!(cause.to_string().contains("`connect_within` is zero"), "the refusal names the setting: {cause}");
    assert_eq!(silent.accepted(), 0, "a zero bound reached the network");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_clock_fails_listen_as_a_permanent_runtime_shut_down() {
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    clock.refuse(WITHIN);
    let error = failed_listen(Nats::url(silent.url()).connect_within(WITHIN), clock.clone(), "listen on a refusing clock").await;
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
    let client = RpcClient::new(Nats::url(silent.url()).connect_within(WITHIN), runtime).expect("the link is usable");
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
    let listening = tokio::spawn(listen_on(Nats::url(silent.url()), clock.clone()));
    until("the unset bound armed and the connection accepted", || clock.live(UNSET) == 1 && silent.accepted() == 1).await;
    clock.advance(UNSET);
    let error = tokio::time::timeout(PATIENCE, listening)
        .await
        .unwrap_or_else(|_| panic!("listen did not return within {PATIENCE:?} of the unset bound passing"))
        .expect("listen did not panic")
        .expect_err("listen succeeded against a NATS server that never answers");
    let cause = bind_cause(error);
    let timed_out = cause.downcast_ref::<ConnectTimedOut>().unwrap_or_else(|| panic!("the `Bind` does not carry `ConnectTimedOut`: {cause:?}"));
    assert_eq!(timed_out.within(), UNSET);
}

/// With both bounds unset, the call's own 5 s timeout ends a call against a silent server before
/// the 30 s connect bound, the connection is closed with it, and the next call connects again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn with_both_bounds_unset_the_call_s_timeout_ends_the_connect_first() {
    const CALL: Duration = Duration::from_secs(5);
    const UNSET: Duration = Duration::from_secs(30);
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let app = app(&clock).await;
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let client = Arc::new(RpcClient::new(Nats::url(silent.url()), runtime).expect("the link is usable"));
    let calling = tokio::spawn({
        let client = Arc::clone(&client);
        async move { client.request::<_, u32>("silent.call", &()).await }
    });
    until("both bounds armed and the connection accepted", || clock.live(CALL) == 1 && clock.live(UNSET) == 1 && silent.accepted() == 1).await;
    clock.advance(CALL);
    let failed = tokio::time::timeout(PATIENCE, calling)
        .await
        .unwrap_or_else(|_| panic!("the call did not end within {PATIENCE:?} of its timeout passing"))
        .expect("the call did not panic")
        .expect_err("a call succeeded against a NATS server that never answers");
    assert_eq!(failed.kind(), ErrorKind::Timeout, "{failed:?}");
    until("the timed-out call's connection closed", || silent.closed() == 1).await;
    let again = tokio::spawn(async move { client.request::<_, u32>("silent.call", &()).await });
    until("the next call connected again", || silent.accepted() == 2).await;
    again.abort();
}
