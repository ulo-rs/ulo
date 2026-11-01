# 38 tracing

W3C trace context across three transports. An HTTP edge calls an RPC service with `.within(&exec)`, which calls a gRPC service with `ulo_grpc::outgoing(&exec, request)`; each hop opens a server span as a child of the span that called it, and the trace id the caller sent arrives unchanged at the last hop. `ulo-otel` exports those spans; `ulo-otel-otlp` sends them to a collector over HTTP.

```
cargo test -p ex-38-tracing
OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318 PORT=7000 cargo run -p ex-38-tracing
curl -H 'traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01' http://127.0.0.1:7000/quote/tea
```

The reply lists the `traceparent` each hop saw. `main` runs the three services as three apps in one process; the edge listens on `PORT`.

Read first:

- `src/lib.rs`: the three services and how the context travels: `.within` for RPC, `outgoing` for gRPC; the HTTP edge reads it from the request.
- `src/main.rs`: the OTLP exporter and the subscriber. The subscriber filters at `INFO`, since the layer exports every span it is given and h2 opens trace-level ones.
- `tests/chain.rs`: an in-memory exporter in place of OTLP; one trace id on every hop, each span's parent the span that called it, and a root span when the caller sent no `traceparent`.
- `tests/support/memory.rs`: the exporter, 20 lines.
- `tests/binary.rs`: runs the compiled binary with no collector listening.

Next: [39 one-app](../39-one-app), which serves all four transports in one lifecycle.
