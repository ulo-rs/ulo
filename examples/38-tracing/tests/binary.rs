use ex_38_tracing::Quoted;
use ex_support::http::{Call, call};
use ex_support::Bin;

#[tokio::test]
async fn the_binary_runs_the_three_services_and_exits_clean_on_sigterm() {
    // No collector listens: the exporter's failures go to a subscriber that prints nothing.
    let mut bin = Bin::server(env!("CARGO_BIN_EXE_ex-38-tracing"), &[("OTEL_SERVICE_NAME", "ex-38")]);
    let addr = bin.addr();
    let caller = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
    let reply = call(addr, Call::get("/quote/tea").header("traceparent", caller)).await;
    let quoted: Quoted = serde_json::from_str(&reply.body).expect("a Quoted body");
    let hops = [&quoted.http_trace, &quoted.rpc_trace, &quoted.grpc_trace];
    for trace in hops {
        assert!(trace.starts_with("00-4bf92f3577b34da6a3ce929d0e0e4736-") && trace.ends_with("-01"), "{trace}");
    }
    // Without `main`'s subscriber every hop would forward the caller's span unchanged.
    let spans: std::collections::BTreeSet<&str> = hops.iter().map(|t| &t[36..52]).chain(["00f067aa0ba902b7"]).collect();
    assert_eq!(spans.len(), 4, "each hop under a span of its own: {hops:?}");
    let exit = bin.terminate();
    assert!(exit.status.success(), "{exit:?}");
    assert_eq!(exit.stderr, "");
}
