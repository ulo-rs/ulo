# #90 — Cover non-HTTP transports across pipeline-segment panics

Merged 2026-05-30 into `master` from `test/non-http-panic-coverage`, commit [`4a8428f`](https://github.com/ulo-rs/ulo/commit/4a8428f30ce67bf2053f3ae838d5c5f35cba2637).

## Summary

Closes the panic-recovery coverage gap on RPC, WS, and gRPC — HTTP had segment-level tests in `panic_recovery.rs`, the others had only handler-panic. Also fixes one WS substrate behavior the new tests caught.

## Tests added

- **RPC (TCP)** — 5 tests (`integration-tests/tests/integration/rpc_tcp.rs`): guard, interceptor, pipe, error_handler, renderer. Asserts the wire envelope and the observer-side `PipelineSegment`.
- **WS** — 5 tests (`integration-tests/tests/integration/ws_panic_recovery.rs`): same five segments. The renderer test uses an `AppError` whose `message()` panics to drive the `safe_render` fallback frame.
- **gRPC** — 1 test (`integration-tests/tests/integration/rpc_grpc_macros.rs`): a panicking chain `ErrorHandler` is skipped and the original handler error passes through unchanged. Verifies the chain-runner's log-and-continue policy specifically.

## Substrate fix (WS)

A guard panic during message dispatch returned `WsError::AuthFailed`, which the `ToniApplication` message callback deliberately drops on the floor (the "guard rejected" silent-drop path). That conflated developer error with intentional rejection — the panic was visible only to observers, never to the client.

The fix routes guard panic through `record_pipeline_panic` so the canonical envelope is rendered and the error-handler chain still gets a claim. Guard *rejection* (`return false`) keeps its silent-drop semantics.

## Test plan

- [x] `cargo test -p integration-tests --test integration` — 212 passed
- [x] `cargo test -p toni --lib` — 47 passed
