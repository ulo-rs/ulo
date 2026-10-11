//! The suite's own machinery, apart from any link: the smol harness's runtime on the threads that
//! run a scenario's tasks, the slots a bounded suite's scenarios share, and the relay.

use std::io::{Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::num::NonZeroUsize;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ulo_rpc_conformance::__private::Slots;
use ulo_rpc_conformance::relay::Relay;
use ulo_rpc_conformance::{Harness, OnSmol, OnTokio};

/// How long a read here waits before it counts as no answer.
const PATIENCE: Duration = Duration::from_secs(5);

/// How long a relay test may take in all, so a relay method that never returns fails the test.
const WHOLE: Duration = Duration::from_secs(30);

/// Runs `test` to its end on a thread of its own with `block_on`, failing if it takes over
/// [`WHOLE`] and passing its panic on.
fn bounded(test: impl Future<Output = ()> + Send + 'static) {
    let (done, finished) = mpsc::channel();
    let runner = thread::spawn(move || {
        futures_executor::block_on(test);
        let _ = done.send(());
    });
    match finished.recv_timeout(WHOLE) {
        Ok(()) => runner.join().expect("the test's thread"),
        Err(mpsc::RecvTimeoutError::Disconnected) => std::panic::resume_unwind(runner.join().expect_err("the test's thread panicked")),
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("the test did not finish within {WHOLE:?}"),
    }
}

#[test]
fn a_task_on_a_smol_worker_finds_the_scenario_s_runtime() {
    let found = OnSmol::block_on(async {
        let runtime = OnSmol::runtime();
        let mut threads = Vec::new();
        // Several at once, so some run on the workers rather than the test's own thread.
        let tasks: Vec<_> = (0..16)
            .map(|_| {
                ulo::spawn_with(&runtime, async {
                    let _ = OnSmol::runtime();
                    thread::sleep(Duration::from_millis(20));
                    thread::current().id()
                })
            })
            .collect();
        for task in tasks {
            threads.push(task.await.expect("the task took the harness's runtime and returned"));
        }
        threads
    });
    let here = thread::current().id();
    assert!(found.iter().any(|id| *id != here), "no task ran on a worker thread: {found:?}");
}

#[test]
fn the_tokio_harness_runs_the_scenario_on_a_tokio_runtime() {
    OnTokio::block_on(async {
        let runtime = OnTokio::runtime();
        let answered = ulo::spawn_with(&runtime, async { 7 }).await;
        assert_eq!(answered.expect("the task returned"), 7);
    });
}

#[test]
fn a_bounded_suite_holds_no_more_slots_than_its_bound() {
    static SLOTS: Slots = Slots::new();
    let two = NonZeroUsize::new(2);
    let first = SLOTS.hold(two).expect("a bound gives a slot");
    let second = SLOTS.hold(two).expect("a bound gives a slot");
    let (taken, waited) = mpsc::channel();
    let third = thread::spawn(move || {
        let slot = SLOTS.hold(two);
        taken.send(()).expect("the test is waiting");
        drop(slot);
    });
    assert!(waited.recv_timeout(Duration::from_millis(200)).is_err(), "a third slot was taken while two of two were held");
    drop(first);
    waited.recv_timeout(PATIENCE).expect("the third slot is taken once one is given back");
    third.join().expect("the third holder's thread");
    drop(second);
}

/// A loopback server that echoes each connection until it ends.
fn echo_server() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a loopback port");
    let addr = listener.local_addr().expect("the echo server's address");
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            thread::spawn(move || {
                let mut buf = [0u8; 64];
                while let Ok(read) = stream.read(&mut buf) {
                    if read == 0 || stream.write_all(&buf[..read]).is_err() {
                        return;
                    }
                }
            });
        }
    });
    addr
}

fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).expect("the relay accepts");
    stream.set_read_timeout(Some(PATIENCE)).expect("a read timeout");
    stream
}

/// What one read answers: the bytes, or the end of the connection as empty.
fn read(stream: &mut TcpStream) -> Vec<u8> {
    let mut buf = [0u8; 64];
    let read = stream.read(&mut buf).expect("the connection answers or ends within the read timeout");
    buf[..read].to_vec()
}

#[test]
fn the_relay_carries_both_ways_and_a_cut_ends_every_connection() {
    bounded(async {
        let relay = Relay::start(echo_server()).await;
        let mut first = connect(relay.addr());
        let mut second = connect(relay.addr());
        for stream in [&mut first, &mut second] {
            stream.write_all(b"ping").expect("the write");
            assert_eq!(read(stream), b"ping", "the echo through the relay");
        }
        assert_eq!(relay.open().await, 2, "the connections the relay carries");
        assert_eq!(relay.cut().await, 2, "the connections the cut closed");
        assert_eq!(read(&mut first), b"", "a connection the cut closed reads its end");
        assert_eq!(relay.open().await, 0, "the connections the relay carries after the cut");
        let mut third = connect(relay.addr());
        third.write_all(b"again").expect("the write");
        assert_eq!(read(&mut third), b"again", "a connection after the cut goes through");
        third.shutdown(Shutdown::Write).expect("the client's end");
        assert_eq!(read(&mut third), b"", "the client's end reaches the upstream and comes back");
    });
}

#[test]
fn a_connection_the_upstream_refuses_is_closed() {
    bounded(async {
        // A port nothing listens on: bound and released.
        let refused = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|listener| listener.local_addr()).expect("a loopback port");
        let relay = Relay::start(refused).await;
        let mut client = connect(relay.addr());
        assert_eq!(read(&mut client), b"", "the client's connection ends when the upstream refuses the relay");
    });
}
