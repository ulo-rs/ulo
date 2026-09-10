# #228 — Route a panicking guard through the error chain

Merged 2026-09-01 into `master` from `fix/guard-panic-reaches-the-chain`, commit [`d2d44d7`](https://github.com/ulo-rs/ulo/commit/d2d44d7ff129b3fd0b3cf97b6c84f76566e483a4).

A guard that panics is a bug, not a verdict, and on RPC and gRPC it was answered as one. The panic exit of the guard loop returned `forbidden` and `PermissionDenied` — the codes that tell a caller its credentials were refused — so a client refreshes its token and retries into the same panic. The rejection exit a few lines below already walks the error chain. The panic exit skipped it, so `#[catch(PanicRecovered)]` was never consulted on either transport.

Both exits now take the same route. An unclaimed guard panic renders `Internal`, which is what HTTP and WebSocket have always rendered for one.

- **RPC** — the panic exit delegates to `record_pipeline_panic`, the helper an interceptor panic already uses: chain first, then `RpcError::from(event)` through the renderer.
- **gRPC** — the panic exit calls `run_grpc_error_chain` with the `PanicRecovered`, falling back to `Internal` carrying the panic message.
- **Tests** — `guard_panic_is_an_event.rs` reshapes the panic into a catcher's own answer on both transports, reading the `guard` segment back off the event. Reverting either dispatcher fails it against the old `forbidden` frame.

Breaking: a guard panic on RPC now arrives as the canonical `Internal` envelope in a wire-success frame rather than a wire-`err` `forbidden` frame, and on gRPC as `Code::Internal` rather than `Code::PermissionDenied`. A guard *rejection* is unchanged on both.

A WebSocket connect guard keeps refusing the upgrade without the chain: there is no answer to shape on a connect, which is the same reason interceptors and error handlers do not run there.

ADR-0035's consequence bullet is rewritten to describe this behaviour.
