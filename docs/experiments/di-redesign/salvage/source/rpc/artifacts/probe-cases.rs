// PROBE CASES — RPC wire audit, 2026-09-23. Temporary; reverted after the run.
// ---------------------------------------------------------------------------

/// Counts every delivery of `probe.fanout`, across every application instance
/// in this process.
pub static FANOUT: AtomicUsize = AtomicUsize::new(0);

#[controller]
pub struct ProbeController {}

#[patterns]
impl ProbeController {
    #[new]
    pub fn new() -> Self {
        Self {}
    }

    /// Names the `RpcData` variant the handler was handed, so a caller can see
    /// what its payload became in transit.
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

async fn with_probe_server<B, F, Fut>(broker: &B, body: F)
where
    B: Broker,
    F: FnOnce(RpcClient) -> Fut,
    Fut: Future<Output = ()>,
{
    let adapter = broker.adapter();
    let client = RpcClient::new(broker.transport());
    tokio::spawn(async move {
        let mut app = UloFactory::new()
            .create_with(ProbeModule)
            .await
            .expect("the probe module builds");
        app.use_rpc_adapter(adapter).expect("adapter accepted");
        app.bind().await.expect("the transport binds");
        app.run().await;
    });
    body(client).await;
}

/// P1 — what a caller sees when no controller declares the pattern.
pub async fn probe_unrouted_pattern<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();
    with_probe_server(&broker, |client| async move {
        within(budget.boot, || async {
            client.send("probe.ready", RpcData::json(serde_json::json!(1))).await.ok()
        })
        .await
        .expect("the transport must be carrying calls first");

        let outcome = client
            .send("nothing.declares.this", RpcData::json(serde_json::json!({})))
            .await;
        println!("PROBE unrouted => {outcome:?}");
    })
    .await;
}

/// P2 — what each `RpcData` variant becomes by the time the handler reads it.
pub async fn probe_payload_variant<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();
    with_probe_server(&broker, |client| async move {
        within(budget.boot, || async {
            client.send("probe.ready", RpcData::json(serde_json::json!(1))).await.ok()
        })
        .await
        .expect("the transport must be carrying calls first");

        for (label, sent) in [
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
            println!("PROBE variant {label} => {seen}");
        }
    })
    .await;
}

/// P3 — one emit, two server instances on one broker: how many handler runs?
pub async fn probe_two_instances_one_emit<B: Broker>() {
    let broker = B::start().await;
    let budget = B::budget();

    let client = RpcClient::new(broker.transport());
    for _ in 0..2 {
        let adapter = broker.adapter();
        tokio::spawn(async move {
            let mut app = UloFactory::new()
                .create_with(ProbeModule)
                .await
                .expect("the probe module builds");
            app.use_rpc_adapter(adapter).expect("adapter accepted");
            app.bind().await.expect("the transport binds");
            app.run().await;
        });
    }

    within(budget.boot, || async {
        client.send("probe.ready", RpcData::json(serde_json::json!(1))).await.ok()
    })
    .await
    .expect("at least one instance must be reachable");
    // Both instances subscribe asynchronously; the first reply proves only one.
    tokio::time::sleep(Duration::from_secs(3)).await;

    FANOUT.store(0, Ordering::SeqCst);
    client
        .emit("probe.fanout", RpcData::json(serde_json::json!({})))
        .await
        .expect("emit accepted");
    tokio::time::sleep(budget.settle).await;
    println!(
        "PROBE fanout => {} handler runs across 2 instances",
        FANOUT.load(Ordering::SeqCst)
    );
}
