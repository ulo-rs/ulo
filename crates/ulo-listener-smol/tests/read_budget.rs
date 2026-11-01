//! The listener's read budget (`SmolStream`'s `READS_PER_TURN`) is what keeps a connection loop
//! that awaits nothing else from holding the executor's thread while a peer's bytes keep arriving
//! (transports DESIGN §3.7). Two cases pin it on a one-thread executor beside a ticker: the yield
//! itself, and the wakeup that brings the connection back from it.

use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures::AsyncReadExt;
use ulo::Runtime;
use ulo_conformance_support::starvation::{Flood, Meter, Probe, Starvation};
use ulo_listener_smol::SmolListener;
use ulo_net::{BoundListener, Endpoint};
use ulo_server_hyper::{Accepted, FuturesIo, Serve, ServeConfig};

const LOOP: &str = "ulo-listener-smol read budget (SmolStream::poll_read yield every READS_PER_TURN reads)";

/// The reads the listener answers between the turns it gives other tasks, as the source has it.
const READS_PER_TURN: u64 = 32;

/// The connection loop's read buffer: small, so a budget's worth of reads is a few bytes and the
/// flood outlasts many budgets.
const READ: usize = 16;

/// Starts a server on port 0 whose connections wait for `go`, then read to their end into
/// `received` and `meter` with a [`READ`]-byte buffer.
fn serve(probe: &Probe, go: Arc<AtomicBool>, received: Arc<AtomicU64>, meter: Meter) -> std::net::SocketAddr {
    let listener = BoundListener::bind(&Endpoint::parse("127.0.0.1:0").expect("an address")).expect("a port");
    let addr = listener.local_addr();
    let runtime: Arc<dyn Runtime> = Arc::new(probe.runtime());
    let serve = Arc::new(Serve::new(vec![listener], None, &ServeConfig::default(), Arc::clone(&runtime)).expect("the smol listener adopts the socket"));
    let waiting = Arc::clone(&runtime);
    drop(runtime.spawn(Box::pin(async move {
        let _ = serve
            .run(move |accepted: Accepted<SmolListener>| {
                let (go, received, meter, waiting) = (Arc::clone(&go), Arc::clone(&received), meter.clone(), Arc::clone(&waiting));
                async move {
                    let mut io = FuturesIo::new(accepted.io);
                    while !go.load(Ordering::Acquire) {
                        waiting.sleep(Duration::from_millis(1)).await;
                    }
                    let mut buffer = [0u8; READ];
                    while let Ok(read @ 1..) = io.read(&mut buffer).await {
                        received.fetch_add(read as u64, Ordering::AcqRel);
                        meter.add(read as u64);
                    }
                }
            })
            .await;
    })));
    addr
}

/// A peer that writes more than a budget's worth in one write and then says nothing leaves the
/// connection no readiness to wake it: the read that answers `Pending` at the budget must wake
/// its own task, or the bytes still buffered are never read.
#[test]
fn a_read_that_yields_at_the_budget_wakes_its_own_task() {
    Starvation::new(LOOP).run(|probe| async move {
        let total = READ as u64 * READS_PER_TURN * 8;
        let (go, received) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let addr = serve(&probe, Arc::clone(&go), Arc::clone(&received), probe.meter());
        let peer = TcpStream::connect(addr).expect("the listener accepts");
        let write = Flood::once(&peer, vec![7; total as usize]);
        probe.settles("the peer's single write ending", || write.ended()).await;
        assert!(write.failure().is_none(), "the peer's write failed");
        go.store(true, Ordering::Release);
        probe.settles(&format!("the connection reading all {total} bytes of one write; a yield at the budget that wakes nothing leaves the rest buffered"), || {
            received.load(Ordering::Acquire) == total
        })
        .await;
        drop(peer);
    });
}

/// The whole flood is in the kernel before the connection's first read, so it never meets a read
/// that would wait: only the budget gives the ticker a turn, and gives it one every
/// `READS_PER_TURN` reads.
#[test]
fn a_flood_already_buffered_does_not_hold_the_executor_past_the_budget() {
    Starvation::new(LOOP).run(|probe| async move {
        let flood = READ as u64 * 2048;
        let (go, received) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let addr = serve(&probe, Arc::clone(&go), Arc::clone(&received), probe.meter());
        let peer = TcpStream::connect(addr).expect("the listener accepts");
        let write = Flood::once(&peer, vec![7; flood as usize]);
        probe.settles("the peer's single write ending", || write.ended()).await;
        assert!(write.failure().is_none(), "the peer's write failed");
        probe.ticks_advance(1).await;
        probe.reset_gap();
        go.store(true, Ordering::Release);
        probe.settles(&format!("all {flood} bytes read"), || received.load(Ordering::Acquire) == flood).await;
        probe.expect_gap_within(READ as u64 * READS_PER_TURN, "bytes");
        drop(peer);
    });
}
