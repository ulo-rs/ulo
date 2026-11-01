//! Q20-P4: a client's calls share the link's one connect, so against a broker that accepts the
//! connection and never answers the AMQP handshake, a burst of calls each ending at its own
//! timeout opens one connection and leaves one lapin `lapin-io-loop` thread, and the next connect
//! starts only once `connect_within` has passed.
//!
//! The threads are counted by name across the whole process, and every connect against a silent
//! broker leaves its thread until the process ends (transports DESIGN §5.2), so this file holds
//! one test.
//!
//! Linux only: the names come from `/proc`, which reads in safe Rust. The one thread per connect
//! is lapin's own behaviour rather than the OS's, so one OS proves it; on any other OS the file
//! builds nothing.

#![cfg(target_os = "linux")]

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use ulo::testing::ScriptedTimer;
use ulo::{App, Bound, Module, ModuleDef, ModuleIdentity};
use ulo_rpc::{ErrorKind, RpcClient};
use ulo_rpc_rabbitmq::RabbitMq;
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

/// The link's bound, and the client's call timeout, which ends each call well inside it.
const WITHIN: Duration = Duration::from_secs(30);
const CALL: Duration = Duration::from_secs(1);

/// The calls started together, and those started one after another once the last has ended.
const BURST: usize = 10;
const SEQUENCE: usize = 10;

/// How long the test waits, in real time, for the link to reach a state it then acts on.
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
}

/// The process's threads named `lapin-io-loop`, read from `/proc/self/task/*/comm`.
fn lapin_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .expect("the process's threads are listed under /proc")
        .flatten()
        .filter(|task| std::fs::read_to_string(task.path().join("comm")).is_ok_and(|name| name.trim_end() == "lapin-io-loop"))
        .count()
}

/// Waits, in real time, until `done` holds, failing naming `what` after [`PATIENCE`].
async fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "{what} did not happen within {PATIENCE:?}");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn calls_timing_out_against_a_silent_broker_leave_one_connect_per_bound() {
    let before = lapin_threads();
    let silent = Silent::start();
    let clock = ScriptedTimer::new();
    let app = App::builder(Root).runtime(Tokio::current()).timer(clock.clone()).wire().expect("the app wires").connect().await.expect("the app connects");
    let runtime = Arc::clone(app.handle().runtime().expect("the app has a runtime"));
    let link = RabbitMq::url(format!("amqp://guest:guest@{}/%2f", silent.addr)).connect_within(WITHIN);
    let client = RpcClient::new(link, runtime).expect("the link is usable").timeout(Bound::After(CALL)).expect("a nonzero timeout");
    let timed_out = |call: Result<u32, ulo_rpc::RpcError>, what: &str| {
        let failed = call.expect_err("a call succeeded against a broker that never answers");
        assert_eq!(failed.kind(), ErrorKind::Timeout, "{what} ended other than at its own timeout: {failed:?}");
    };

    let burst: Vec<_> = (0..BURST)
        .map(|_| {
            let client = client.clone();
            tokio::spawn(async move { client.request::<_, u32>("silent.call", &()).await })
        })
        .collect();
    until("the burst's timeouts armed and its connect accepted", || clock.live(CALL) == BURST && silent.accepted() == 1).await;
    clock.advance(CALL);
    for call in burst {
        timed_out(tokio::time::timeout(PATIENCE, call).await.expect("a call of the burst ended").expect("the call did not panic"), "a call of the burst");
    }
    assert_eq!(silent.accepted(), 1, "the burst's calls opened more than one connection");

    for _ in 0..SEQUENCE {
        let caller = client.clone();
        let call = tokio::spawn(async move { caller.request::<_, u32>("silent.call", &()).await });
        until("the call's timeout armed", || clock.live(CALL) == 1).await;
        clock.advance(CALL);
        timed_out(tokio::time::timeout(PATIENCE, call).await.expect("a later call ended").expect("the call did not panic"), "a later call");
    }
    assert_eq!(silent.accepted(), 1, "a call after the burst connected again inside the bound");
    assert_eq!(lapin_threads() - before, 1, "{} calls that timed out left more than one lapin thread", BURST + SEQUENCE);

    // The bound passes under a call waiting longer than it: the call fails naming the bound, and
    // only the call after it connects again.
    let moved = CALL * u32::try_from(1 + SEQUENCE).expect("a small count");
    let caller = client.clone();
    let waiting = tokio::spawn(async move { caller.request::<_, u32>("silent.call", &()).timeout(WITHIN * 2).await });
    until("the waiting call's timeout armed", || clock.live(WITHIN * 2) == 1).await;
    clock.advance(WITHIN - moved);
    let failed = tokio::time::timeout(PATIENCE, waiting)
        .await
        .expect("the waiting call ended at the bound")
        .expect("the call did not panic")
        .expect_err("a call succeeded against a broker that never answers");
    assert_eq!((failed.kind(), failed.reason()), (ErrorKind::Unavailable, Some("connect_timed_out")), "{failed:?}");
    assert!(failed.message().contains("connect_within"), "the call waiting at the bound does not name it: {}", failed.message());
    assert_eq!(silent.accepted(), 1, "the link connected again before a call asked it to");

    let caller = client.clone();
    let retried = tokio::spawn(async move { caller.request::<_, u32>("silent.call", &()).await });
    until("the next call's connect accepted", || silent.accepted() == 2).await;
    assert_eq!(lapin_threads() - before, 2, "the second bound's connect left other than one more lapin thread");
    until("the next call's timeout armed", || clock.live(CALL) == 1).await;
    clock.advance(CALL);
    timed_out(tokio::time::timeout(PATIENCE, retried).await.expect("the next call ended").expect("the call did not panic"), "the next call");
}
