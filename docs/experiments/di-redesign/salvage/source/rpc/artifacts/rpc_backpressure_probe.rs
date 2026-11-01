//! D8 probe — does anything bound what a reply stream buffers on the client?
//!
//! The chain is producer -> ScopedStream -> drive_reply_stream -> socket ->
//! reader_loop -> `PendingSlot::Stream(UnboundedSender)` -> forward_stream ->
//! `RpcReplyStream::channel(32)` -> caller. Every link is bounded except the
//! unbounded queue in the pending map, which is downstream of the socket and
//! upstream of the only declared bound.
//!
//! The measurement is the gap between what the handler produced and what the
//! caller pulled. A gap that plateaus means something applies backpressure; a
//! gap that grows with time means nothing does.
//!
//! The control is the same case with a consumer that reads flat out. If the
//! gap grows in both, the harness is measuring the producer's speed rather
//! than the buffer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use ulo::UloFactory;
use ulo::dispatch::Items;
use ulo::rpc::{RpcClient, RpcData, RpcError, RpcHandlerResult};
use ulo_macros::{controller, message_pattern, module, new, patterns};

/// Items the handler has handed to the stream.
static PRODUCED: AtomicUsize = AtomicUsize::new(0);

/// One item's payload. Big enough that the gap is visible as memory, small
/// enough that an unbounded run does not take the machine down.
const ITEM_BYTES: usize = 1024;
/// Hard stop so a proven-unbounded run cannot exhaust the host.
const MAX_ITEMS: usize = 200_000;

#[controller]
pub struct FloodController {}

#[patterns]
impl FloodController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Produces until the cap, as fast as whatever is downstream will take.
    #[message_pattern("flood")]
    async fn flood(&self, _d: RpcData) -> RpcHandlerResult {
        let payload = "x".repeat(ITEM_BYTES);
        let stream = futures_util::stream::repeat_with(move || {
            PRODUCED.fetch_add(1, Ordering::SeqCst);
            Ok(RpcData::json(serde_json::json!(payload.clone())))
        })
        .take(MAX_ITEMS)
        .boxed();
        Ok(Items::Many(stream))
    }

    #[message_pattern("ready")]
    async fn ready(&self, _d: RpcData) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!(true)))
    }
}

#[module(controllers: [FloodController])]
impl FloodModule {}

async fn start() -> u16 {
    let (tx, rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = UloFactory::new().create_with(FloodModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.rpc.unwrap().port());
        app.run().await;
    });
    rx.await.unwrap()
}

/// Resident set size in KiB, read from `ps`. Coarse on purpose — the claim is
/// about an order of magnitude, not a byte count.
fn rss_kib() -> usize {
    std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

/// Run one case and report. `read_every` is the consumer's pace.
async fn measure(label: &str, read_every: Option<Duration>, run_for: Duration) {
    PRODUCED.store(0, Ordering::SeqCst);
    let port = start().await;
    let client = RpcClient::new(
        ulo_rpc_tcp::TcpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_secs(30)),
    );
    client
        .send("ready", RpcData::json(serde_json::json!(1)))
        .await
        .expect("the server answers before the flood means anything");

    let rss_before = rss_kib();
    let mut stream = client
        .stream("flood", RpcData::json(serde_json::json!(null)))
        .await
        .expect("the flood stream opens");

    let started = Instant::now();
    let mut consumed = 0usize;
    let mut samples: Vec<(u64, usize, usize)> = Vec::new();
    let mut next_sample = Duration::from_secs(1);

    while started.elapsed() < run_for {
        if let Some(pace) = read_every {
            tokio::time::sleep(pace).await;
        }
        match tokio::time::timeout(Duration::from_millis(50), stream.next()).await {
            Ok(Some(_)) => consumed += 1,
            Ok(None) => break,
            Err(_) => {}
        }
        if started.elapsed() >= next_sample {
            let p = PRODUCED.load(Ordering::SeqCst);
            samples.push((next_sample.as_secs(), p, consumed));
            next_sample += Duration::from_secs(1);
        }
    }

    let produced = PRODUCED.load(Ordering::SeqCst);
    let rss_after = rss_kib();
    println!("PROBE[{label}]");
    for (sec, p, c) in &samples {
        println!(
            "  t={sec}s produced={p} consumed={c} gap={} (~{} KiB held)",
            p - c,
            (p - c) * ITEM_BYTES / 1024
        );
    }
    println!(
        "  final produced={produced} consumed={consumed} gap={} rss {}->{} KiB (delta {} KiB)",
        produced - consumed,
        rss_before,
        rss_after,
        rss_after as i64 - rss_before as i64
    );
    drop(stream);
}

/// The case: a consumer reading one item every 100 ms against a producer that
/// is not paced.
#[tokio::test(flavor = "multi_thread")]
async fn probe_slow_consumer_is_unbounded() {
    measure(
        "slow consumer, 1 item/100ms",
        Some(Duration::from_millis(100)),
        Duration::from_secs(4),
    )
    .await;
}

/// The control: the same producer against a consumer reading flat out. A gap
/// that grows here too would mean the harness measures production speed.
#[tokio::test(flavor = "multi_thread")]
async fn probe_fast_consumer_is_the_control() {
    measure("fast consumer, no pacing", None, Duration::from_secs(4)).await;
}
