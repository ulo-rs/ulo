//! The linger a server's `close` leaves on each connection (F484) reads and discards what the peer
//! still sends until the peer closes its half or `CUT_GRACE` passes. Its loop must hand the
//! executor a turn every `READS_PER_TURN` reads: on smol a read answers at once while the bytes keep
//! arriving, so a loop that awaits nothing else would hold its thread, and the grace's deadline,
//! polled by the same task after the loop, would never be reached.
//!
//! The case runs on a one-thread executor beside a ticker (`ulo-conformance-support`'s
//! `starvation`). The server's reads wait at a gate (`gated`): the connection's own read parks
//! there, the link closes, the linger's first read parks there, and only then does the peer write
//! its flood, so the whole flood is in the kernel when the linger first reads.
//! Each read hands over at most `CAP` bytes, so the bytes read between two turns of the ticker are
//! the reads the linger made between them: a budget's worth with the yield, the whole flood
//! without it. The peer then goes silent and keeps its half open, and the linger ends where the
//! scripted clock reaches the grace, not a tick before.

use std::net::TcpStream;
use std::time::Duration;

use ulo_conformance_support::starvation::{Flood, Probe, Starvation};
use ulo_rpc_tcp_core::CUT_GRACE;

mod gated;

use gated::CAP;

const LOOP: &str = "ulo-rpc-tcp-core linger (Reading::discard, yield every READS_PER_TURN reads)";

/// The reads between the yields, as the source has it.
const READS_PER_TURN: u64 = 32;

/// Bytes the peer writes: well inside the two sockets' buffers, and many budgets of `CAP`-byte reads.
const FLOOD: usize = 32 * 1024;

#[test]
fn a_peer_writing_after_close_began_does_not_hold_the_executor_through_the_linger() {
    Starvation::new(LOOP).run(run);
}

async fn run(probe: Probe) {
    let server = gated::serve(&probe);
    let peer = TcpStream::connect(server.addr).expect("the link accepts a connection");
    probe.settles("the connection's first read reaching the gate", || server.gate.parks() >= 1).await;

    server.core.close().await;
    probe.settles("the link asking the app's clock for its grace", || server.clock.live(CUT_GRACE) == 1).await;
    probe.settles("the linger's first read reaching the gate", || server.gate.parks() >= 2).await;

    let write = Flood::once(&peer, vec![0; FLOOD]);
    probe.settles("the peer's single write ending", || write.ended()).await;
    assert!(write.failure().is_none(), "{LOOP}: the peer's write failed");
    probe.ticks_advance(1).await;
    probe.reset_gap();
    server.gate.release();

    probe.settles("the linger reading the whole flood", || probe.metered() == FLOOD as u64).await;
    probe.expect_gap_within(READS_PER_TURN * CAP as u64, "bytes");

    server.clock.advance(CUT_GRACE - Duration::from_millis(1));
    probe.ticks_advance(100).await;
    assert_eq!(server.clock.live(CUT_GRACE), 1, "{LOOP}: the linger ended a millisecond short of the grace");
    server.clock.advance(Duration::from_millis(1));
    probe.settles("the linger ending at the grace", || server.clock.live(CUT_GRACE) == 0).await;
    drop(peer);
    drop(server.inbound);
}
