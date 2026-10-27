//! A server connection's frame loop hands the executor a turn every `READS_PER_TURN` frames even
//! when nothing it reads awaits anything. A peer writing `cancel` frames that name no call has
//! them read and dropped without a send to a queue that could fill, and on smol a read answers at
//! once while the bytes keep arriving, so the yield in `Reading::turn` is the loop's only wait.
//!
//! The case runs on a one-thread executor beside a ticker (`ulo-conformance-support`'s
//! `starvation`). The server's reads wait at a gate (`gated`) until the peer has written its whole
//! flood, so the flood is in the kernel when the loop first reads. Each read hands over at most
//! `CAP` bytes, so the bytes read between two turns of the ticker are the frames the loop took
//! between them: a budget's worth with the yield, the whole flood without it.

use std::net::TcpStream;

use ulo_conformance_support::starvation::{Flood, Probe, Starvation};
use ulo_rpc::{Codec, Frame};

mod gated;

const LOOP: &str = "ulo-rpc-tcp-core server frame loop (Reading::turn in Reading::frame, yield every READS_PER_TURN frames)";

/// The frames between the yields, as the source has it.
const READS_PER_TURN: u64 = 32;

/// Frames the peer writes: many budgets, inside the two sockets' buffers.
const FRAMES: usize = 1024;

#[test]
fn a_flood_of_dropped_frames_does_not_hold_the_executor() {
    Starvation::new(LOOP).run(run);
}

async fn run(probe: Probe) {
    let server = gated::serve(&probe);
    let peer = TcpStream::connect(server.addr).expect("the link accepts a connection");
    probe.settles("the connection's first read reaching the gate", || server.gate.parks() >= 1).await;

    let cancel = Codec::Json.encode_frame(&Frame::Cancel { id: u64::MAX }).expect("the frame encodes");
    let frame = 4 + cancel.len();
    let mut flood = Vec::with_capacity(FRAMES * frame);
    for _ in 0..FRAMES {
        flood.extend_from_slice(&(cancel.len() as u32).to_be_bytes());
        flood.extend_from_slice(&cancel);
    }
    let total = flood.len() as u64;
    let write = Flood::once(&peer, flood);
    probe.settles("the peer's single write ending", || write.ended()).await;
    assert!(write.failure().is_none(), "{LOOP}: the peer's write failed");
    probe.ticks_advance(1).await;
    probe.reset_gap();
    server.gate.release();

    probe.settles(&format!("the loop reading all {total} bytes of the flood"), || probe.metered() == total).await;
    probe.expect_gap_within(READS_PER_TURN * frame as u64, "bytes");
    drop(server.inbound);
}
