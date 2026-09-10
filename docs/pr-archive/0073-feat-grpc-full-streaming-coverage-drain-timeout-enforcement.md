# #73 — feat(grpc): full streaming coverage + drain-timeout enforcement

Merged 2026-05-02 into `master` from `feat/grpc-streaming`, commit [`9c3087b`](https://github.com/ulo-rs/ulo/commit/9c3087b9480a0c1cec4a6e4445f9753eaa9229d7).

The `#[grpc_methods]` macro doesn't inspect individual method signatures — it just wraps the user's trait impl in `*Server::new(self.clone())` and lets tonic dispatch. Hypothesis: streaming should already work through the macros. This PR proves it across all three streaming modes, then closes the drain-timeout loop that was left stored-but-unused since the gRPC seam landed.

## Streaming coverage (no source changes)

Three new tests exercise the remaining gRPC call modes through real gRPC clients:

- `grpc_server_streaming_round_trip` — `WatchProgress` emits a sequence of `ProgressEvent` items.
- `grpc_client_streaming_round_trip` — `BulkCreate` ingests a stream of orders and replies with a summary.
- `grpc_bidi_streaming_round_trip` — `Chat` echoes each inbound message back tagged with a counter-assigned id.

The injected `OrdersCounter` increments across stream items, confirming DI flows through every call mode the same way it does for unary. `#[grpc_methods]`, `#[grpc_service]`, and the `toni-grpc` adapter source all stay unchanged in the streaming commit.

## Drain-timeout enforcement

Tonic's `serve_with_incoming_shutdown` waits for in-flight RPCs to drain naturally — fine for unary, fatal for a bidi stream the client never closes. `with_drain_timeout` has been stored on `GrpcAdapter` since the seam, but until now it wasn't enforced.

The serve future now races tonic's drain against a deadline that starts ticking only when shutdown is signalled — a global `tokio::time::timeout` would cap total uptime instead of drain time. When the budget elapses the serve future is dropped, hyper closes connections, and in-flight streams see `UNAVAILABLE`. Setting the timeout to `None` preserves the wait-forever behaviour.

`grpc_drain_timeout_aborts_long_running_streams` opens a bidi stream the client never closes, fires shutdown, and asserts `completed()` resolves between the budget and a generous upper bound — proving both that the timer fires only after the signal *and* that it actually aborts the stuck handler.

## Out of scope

- Guard / interceptor reuse on gRPC handlers.
