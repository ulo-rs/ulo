//! A server connection reading past a frame over its `max_frame` hands the executor a turn every
//! `READS_PER_TURN` reads, as its frame loop does (F485). The frame's bytes past its leading ones
//! are read and dropped by a loop whose reads, on smol, answer at once while the bytes keep
//! arriving, so the yield in `Reading::turn` is the loop's only wait.
//!
//! The case runs on a one-thread executor beside a ticker (`ulo-conformance-support`'s
//! `starvation`). The server's reads wait at a gate (`gated`) until the peer has written the whole
//! frame, so it is in the kernel when the loop first reads, and each read hands over at most `CAP`
//! bytes. Between two turns of the ticker the loop reads at most the frame's prefix and leading
//! bytes, which it reads whole, and one budget of reads past them; without the yield it reads the
//! whole frame.

use std::net::TcpStream;

use ulo_conformance_support::starvation::{Flood, Probe, Starvation};

mod gated;

use gated::CAP;

const LOOP: &str = "ulo-rpc-tcp-core over-limit read (Reading::turn in Reading::over, yield every READS_PER_TURN reads)";

/// The reads between the yields, and the leading bytes read whole before the first, as the source
/// has them.
const READS_PER_TURN: u64 = 32;
const LEADING: u64 = 4096;

/// The server's `max_frame`, and a frame past it that fits the two sockets' buffers.
const LIMIT: u64 = 1024;
const FRAME: usize = 48 * 1024;

/// Planted break: `Reading::over` reading past the leading bytes with no `turn`.
#[test]
fn an_over_limit_frame_read_past_does_not_hold_the_executor() {
    Starvation::new(LOOP).run(run);
}

async fn run(probe: Probe) {
    let server = gated::serve_limited(&probe, Some(LIMIT));
    let peer = TcpStream::connect(server.addr).expect("the link accepts a connection");
    probe.settles("the connection's first read reaching the gate", || server.gate.parks() >= 1).await;

    let mut flood = (FRAME as u32).to_be_bytes().to_vec();
    flood.extend_from_slice(br#"{"t":"evt","p":"p","h":{},"d":""#);
    flood.resize(4 + FRAME - 2, b'x');
    flood.extend_from_slice(br#""}"#);
    let total = flood.len() as u64;
    let write = Flood::once(&peer, flood);
    probe.settles("the peer's single write ending", || write.ended()).await;
    assert!(write.failure().is_none(), "{LOOP}: the peer's write failed");
    probe.ticks_advance(1).await;
    probe.reset_gap();
    server.gate.release();

    probe.settles(&format!("the loop reading all {total} bytes of the frame"), || probe.metered() == total).await;
    probe.expect_gap_within(4 + LEADING + READS_PER_TURN * CAP as u64, "bytes");
    drop(server.inbound);
}
