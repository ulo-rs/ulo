//! RPC wire audit probe, 2026-09-23. Temporary; not part of the suite.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use ulo::UloFactory;
use ulo::rpc::{RpcClient, RpcData, RpcError};
use ulo_macros::{controller, event_pattern, message_pattern, module, new, patterns};

static FANOUT: AtomicUsize = AtomicUsize::new(0);

#[controller]
pub struct ProbeController {}

#[patterns]
impl ProbeController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    #[message_pattern("probe.variant")]
    async fn variant(&self, data: RpcData) -> Result<RpcData, RpcError> {
        let name = match &data {
            RpcData::Json(v) => format!("Json({v})"),
            RpcData::Text(s) => format!("Text({s})"),
            RpcData::Binary(b) => format!("Binary({})", String::from_utf8_lossy(b)),
        };
        Ok(RpcData::json(serde_json::json!({ "saw": name })))
    }

    #[event_pattern("probe.fanout")]
    async fn fanout(&self, _d: RpcData) -> Result<(), RpcError> {
        FANOUT.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    #[message_pattern("probe.ready")]
    async fn ready(&self, _d: RpcData) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!(true)))
    }
}

#[module(controllers: [ProbeController])]
impl ProbeModule {}

async fn start_tcp() -> u16 {
    let (tx, rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = UloFactory::new().create_with(ProbeModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.rpc.unwrap().port());
        app.run().await;
    });
    rx.await.unwrap()
}

async fn start_udp() -> u16 {
    let (tx, rx) = tokio::sync::oneshot::channel::<u16>();
    tokio::spawn(async move {
        let mut app = UloFactory::new().create_with(ProbeModule).await.unwrap();
        app.use_rpc_adapter(ulo_rpc_udp::UdpAdapter::new("127.0.0.1", 0))
            .unwrap();
        let bound = app.bind().await.unwrap();
        let _ = tx.send(bound.rpc.unwrap().port());
        app.run().await;
    });
    rx.await.unwrap()
}

async fn run_probes(label: &str, client: RpcClient) {
    let outcome = client
        .send("nothing.declares.this", RpcData::json(serde_json::json!({})))
        .await;
    println!("PROBE[{label}] unrouted => {outcome:?}");

    for (name, sent) in [
        ("Text(hello)", RpcData::text("hello")),
        ("Text({\"a\":1})", RpcData::text("{\"a\":1}")),
        ("Binary({\"a\":1})", RpcData::binary(b"{\"a\":1}".to_vec())),
        ("Binary(0xFF 0xFE)", RpcData::binary(vec![0xFF, 0xFE])),
        ("Json(\"hello\")", RpcData::json(serde_json::json!("hello"))),
    ] {
        let got = client.send("probe.variant", sent).await;
        let seen = got
            .as_ref()
            .ok()
            .and_then(|d| d.as_json().and_then(|v| v["saw"].as_str().map(String::from)))
            .unwrap_or_else(|| format!("{got:?}"));
        println!("PROBE[{label}] variant {name} => {seen}");
    }
}

#[tokio::test]
async fn probe_tcp() {
    let port = start_tcp().await;
    let client = RpcClient::new(
        ulo_rpc_tcp::TcpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_secs(2)),
    );
    run_probes("tcp", client).await;
}

#[tokio::test]
async fn probe_udp() {
    let port = start_udp().await;
    let client = RpcClient::new(
        ulo_rpc_udp::UdpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_secs(2)),
    );
    run_probes("udp", client).await;
}

/// A single-reply call the caller abandons: does the server hear anything?
#[tokio::test]
async fn probe_abandoned_single_call_is_silent() {
    let port = start_tcp().await;
    let client = RpcClient::new(
        ulo_rpc_tcp::TcpClientTransport::new("127.0.0.1", port)
            .with_timeout(Duration::from_millis(200)),
    );
    let before = FANOUT.load(Ordering::SeqCst);
    let _ = client.send("probe.slow", RpcData::json(serde_json::json!({}))).await;
    println!("PROBE[tcp] abandoned single call: fanout unchanged = {}", FANOUT.load(Ordering::SeqCst) == before);
}

/// An unterminated line: does the reader bound what it buffers?
#[tokio::test]
async fn probe_tcp_unterminated_line() {
    use tokio::io::AsyncWriteExt;
    let port = start_tcp().await;
    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let chunk = vec![b'x'; 1024 * 1024];
    let mut written = 0usize;
    for _ in 0..64 {
        match sock.write_all(&chunk).await {
            Ok(()) => written += chunk.len(),
            Err(e) => {
                println!("PROBE[tcp] unterminated line refused after {written} bytes: {e}");
                return;
            }
        }
    }
    let _ = sock.flush().await;
    println!("PROBE[tcp] unterminated line accepted {written} bytes with no terminator and no refusal");
}

// --- duplicate pattern across two controllers -----------------------------

#[controller]
pub struct DupeA {}

#[patterns]
impl DupeA {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
    #[message_pattern("dupe.pattern")]
    async fn a(&self, _d: RpcData) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("A")))
    }
}

#[controller]
pub struct DupeB {}

#[patterns]
impl DupeB {
    #[new]
    pub fn new() -> Self {
        Self {}
    }
    #[message_pattern("dupe.pattern")]
    async fn b(&self, _d: RpcData) -> Result<RpcData, RpcError> {
        Ok(RpcData::json(serde_json::json!("B")))
    }
}

#[module(controllers: [DupeA, DupeB])]
impl DupeModule {}

#[tokio::test]
async fn probe_duplicate_pattern() {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<u16, String>>();
    tokio::spawn(async move {
        let app = UloFactory::new().create_with(DupeModule).await;
        let mut app = match app {
            Ok(a) => a,
            Err(e) => {
                let _ = tx.send(Err(format!("create refused: {e}")));
                return;
            }
        };
        app.use_rpc_adapter(ulo_rpc_tcp::TcpAdapter::new("127.0.0.1", 0))
            .unwrap();
        match app.bind().await {
            Ok(b) => {
                let _ = tx.send(Ok(b.rpc.unwrap().port()));
                app.run().await;
            }
            Err(e) => {
                let _ = tx.send(Err(format!("bind refused: {e}")));
            }
        }
    });
    match rx.await.unwrap() {
        Err(why) => println!("PROBE[tcp] duplicate pattern => refused: {why}"),
        Ok(port) => {
            let client = RpcClient::new(
                ulo_rpc_tcp::TcpClientTransport::new("127.0.0.1", port)
                    .with_timeout(Duration::from_secs(2)),
            );
            let got = client
                .send("dupe.pattern", RpcData::json(serde_json::json!({})))
                .await;
            println!("PROBE[tcp] duplicate pattern => accepted, answered {got:?}");
        }
    }
}
