# #72 — feat(grpc): per-request rpc.request span via tower::Layer

Merged 2026-05-02 into `master` from `feat/grpc-tracing`, commit [`2c314da`](https://github.com/ulo-rs/ulo/commit/2c314da128b7e0c86012aa60a51cb056bc6859cb).

The TCP and UDP RPC adapters already wrap each inbound call in an `rpc.request` span. The gRPC adapter didn't, so handler logs surfaced bare with no way to correlate them back to the originating call. This installs the same span shape for gRPC, leaving every Toni RPC transport on one observability contract.

## Span fields

- `transport = "grpc"` — same key TCP and UDP use; lets you query across transports uniformly.
- `pattern` — the proto method path (`package.Service/Method`).
- `id` — best-effort correlation id from `x-request-id` or `grpc-trace-id`. gRPC has no native correlation id; whichever header the caller chose is what surfaces.
- `peer` — remote address, when tonic's `TcpConnectInfo` extension is present.

A handler emitting `tracing::info!(item, qty, "handler called")` now produces:

```text
INFO rpc.request{transport="grpc" pattern=toni_test.orders.Orders/Create id=Some("req-1") peer=127.0.0.1:54321}: handler called item=keyboard qty=3
```

— same shape as TCP/UDP, just with `transport="grpc"` and the proto pattern.

## Example

`rpc_tracing` now runs all three transports on a shared `LocalSet` (TCP 4000, UDP 4001, gRPC 5000) so operators can compare the log output side by side. Wiring this up requires a `build.rs` and `proto/orders.proto` in the examples crate, mirroring the integration-tests setup.

## Tests

The existing `rpc_grpc_macros::grpc_service_macro_di_round_trip` test exercises a real gRPC `Create` call with the layer installed — proves dispatch still works. Asserting the captured span fields would mean spinning up a tracing subscriber in-process; that's heavy ceremony for a feature whose whole point is "the operator's existing subscriber sees it." Same call we made on the TCP/UDP tracing PR.

## Out of scope

- Streaming RPCs — span shape carries through unchanged, but streaming-specific testing belongs with the streaming PR.
- Guard / interceptor reuse on gRPC handlers.
