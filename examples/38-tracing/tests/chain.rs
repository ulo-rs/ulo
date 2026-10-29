//! One request through three services: an HTTP edge calls an RPC service, which calls a gRPC
//! service. The caller's `traceparent` enters at the edge; every hop's span is exported to memory.

#[path = "support/memory.rs"]
mod memory;

use ex_38_tracing::{Quoted, edge, pricing, stock};
use ex_support::http::{Call, call};
use ex_support::{Served, serve};
use memory::Collected;
use opentelemetry::trace::{SpanId, SpanKind, TraceId};
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use ulo::AppBuilder;
use ulo_otel::{Otel, OtelOptions};
use ulo_runtime_tokio::Tokio;

const TRACE_ID: &str = "4bf92f3577b34da6a3ce929d0e0e4736";
const CALLER_SPAN: &str = "00f067aa0ba902b7";

async fn listen(app: AppBuilder, server: impl ulo::Server) -> Served {
    let app = app.runtime(Tokio::current()).wire().expect("wires").connect().await.expect("connects");
    serve(app.bind(server).listen().await.expect("listens"))
}

fn ids(traceparent: &str) -> (String, String) {
    let parts: Vec<&str> = traceparent.split('-').collect();
    assert_eq!((parts.len(), parts[0], parts[3]), (4, "00", "01"), "{traceparent}");
    (parts[1].to_owned(), parts[2].to_owned())
}

/// The subscriber is the test thread's own, and the whole chain runs on that thread.
#[tokio::test(flavor = "current_thread")]
async fn one_trace_id_runs_through_http_rpc_and_grpc_and_each_span_is_its_caller_s_child() {
    let spans = Collected::default();
    let otel = Otel::new(OtelOptions::default(), spans.clone()).expect("the exporter builds");
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(LevelFilter::INFO).with(otel.layer()));
    tracing::callsite::rebuild_interest_cache();

    let grpc = listen(pricing(&otel), ulo_grpc::Server::new("127.0.0.1:0")).await;
    let rpc = listen(stock(&otel, grpc.addr), ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("127.0.0.1:0"))).await;
    let http = listen(edge(&otel, rpc.addr), ulo_http_hyper::Server::new("127.0.0.1:0")).await;

    let caller = format!("00-{TRACE_ID}-{CALLER_SPAN}-01");
    let reply = call(http.addr, Call::get("/quote/tea").header("traceparent", &caller)).await;
    assert_eq!(reply.status, 200, "{}", reply.body);
    let quoted: Quoted = serde_json::from_str(&reply.body).expect("a Quoted body");
    assert_eq!((quoted.sku.as_str(), quoted.cents), ("tea", 103));

    // Each hop saw the trace id the caller sent, under a span of its own.
    let seen = [&quoted.http_trace, &quoted.rpc_trace, &quoted.grpc_trace].map(|t| ids(t));
    assert!(seen.iter().all(|(trace, _)| trace == TRACE_ID), "{seen:?}");
    let [(_, http_span), (_, rpc_span), (_, grpc_span)] = seen;
    assert!(http_span != CALLER_SPAN && http_span != rpc_span && rpc_span != grpc_span, "three distinct spans: {http_span} {rpc_span} {grpc_span}");

    http.stop().await.expect("closes");
    rpc.stop().await.expect("closes");
    grpc.stop().await.expect("closes");

    let exported = spans.spans();
    assert_eq!(exported.len(), 3, "one span per call served: {exported:?}");
    let by_span = |id: &str| {
        exported.iter().find(|s| s.span_context.span_id().to_string() == id).unwrap_or_else(|| panic!("no exported span {id}: {exported:?}"))
    };
    let (edge_span, rpc_exported, grpc_exported) = (by_span(&http_span), by_span(&rpc_span), by_span(&grpc_span));
    for span in [edge_span, rpc_exported, grpc_exported] {
        assert_eq!(span.span_context.trace_id(), TraceId::from_hex(TRACE_ID).unwrap());
        assert_eq!(span.span_kind, SpanKind::Server);
    }
    assert_eq!(edge_span.parent_span_id, SpanId::from_hex(CALLER_SPAN).unwrap());
    assert!(edge_span.parent_span_is_remote);
    assert_eq!(rpc_exported.parent_span_id.to_string(), http_span, "the RPC span is the edge span's child");
    assert!(rpc_exported.parent_span_is_remote);
    assert_eq!(grpc_exported.parent_span_id.to_string(), rpc_span, "the gRPC span is the RPC span's child");
    assert!(grpc_exported.parent_span_is_remote);
}

/// With no `traceparent` the edge starts the trace: a root span, and the same new trace id below.
#[tokio::test(flavor = "current_thread")]
async fn a_request_without_a_traceparent_starts_a_trace_at_the_edge() {
    let spans = Collected::default();
    let otel = Otel::new(OtelOptions::default(), spans.clone()).expect("the exporter builds");
    let _guard = tracing::subscriber::set_default(tracing_subscriber::registry().with(LevelFilter::INFO).with(otel.layer()));
    tracing::callsite::rebuild_interest_cache();

    let grpc = listen(pricing(&otel), ulo_grpc::Server::new("127.0.0.1:0")).await;
    let rpc = listen(stock(&otel, grpc.addr), ulo_rpc::Server::new(ulo_rpc_tcp::Tcp::new("127.0.0.1:0"))).await;
    let http = listen(edge(&otel, rpc.addr), ulo_http_hyper::Server::new("127.0.0.1:0")).await;

    let reply = call(http.addr, Call::get("/quote/tea")).await;
    let quoted: Quoted = serde_json::from_str(&reply.body).expect("a Quoted body");
    let [(t1, s1), (t2, s2), (t3, s3)] = [&quoted.http_trace, &quoted.rpc_trace, &quoted.grpc_trace].map(|t| ids(t));
    assert!(t1 == t2 && t2 == t3 && t1 != "0".repeat(32), "one new trace id on every hop");

    http.stop().await.expect("closes");
    rpc.stop().await.expect("closes");
    grpc.stop().await.expect("closes");
    let exported = spans.spans();
    let by_span = |id: &str| exported.iter().find(|s| s.span_context.span_id().to_string() == id).expect("exported");
    assert_eq!(exported.len(), 3);
    assert_eq!(by_span(&s1).parent_span_id, SpanId::INVALID, "the edge span is a root");
    assert_eq!(by_span(&s2).parent_span_id.to_string(), s1);
    assert_eq!(by_span(&s3).parent_span_id.to_string(), s2);
}
