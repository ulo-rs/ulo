# #84 — feat(core): panic recovery for pipes across all transports

Merged 2026-05-30 into `master` from `feat/panic-coverage-pipes`, commit [`885ce6b`](https://github.com/ulo-rs/ulo/commit/885ce6b9c1f212240a2c6f9edde08ac2ce14d7bf).

## Summary

Closes the pipe segment of the pipeline-panic coverage work. `pipe.process(&mut C)` is the only remaining pre-handler segment that was still propagating panics unprotected — guards and interceptors landed in #82. A panic from a user-written pipe (validation, transform, instrumentation) now surfaces through the same observer + chain pipeline as the other segments rather than tearing down the request.

## Components

**`PipelineSegment::Pipe`** (new variant)
- Added alongside `Middleware` / `Guard` / `HandlerBody` etc. with `as_str() = "pipe"`. Observers can distinguish a pipe panic from an interceptor one — the discriminator on `PanicRecovered` was always meant to carry that distinction; until this PR there was no producer of `Pipe`.

**Dispatcher wrap-ups** (HTTP / RPC / WS)
- Each `pipe.process(&mut C)` call is wrapped via `panic_recovery::catch_sync(PipelineSegment::Pipe, ...)`.
- Policy mirrors the interceptor-panic policy: observers see `PanicRecovered { during: Pipe }`, the chain's error handlers get first claim, fallback is the transport's default Internal envelope. Remaining pipes and the handler are skipped — a pipe panic blocks the handler the same way `should_abort()` does, since the dispatcher can't trust the context after a partial run.
- The HTTP / RPC `record_interceptor_panic` helpers are renamed `record_pipeline_panic` since they're now reused for both pre-handler segments.

**Tests**
- HTTP: one new test in `panic_recovery.rs` — panicking pipe renders 500, observer sees `PipelineSegment::Pipe`, panic message bubbles into the wire envelope.
- 198/198 integration tests pass.

## Deferred to follow-up

Two pipeline segments still propagate panics unprotected:
- **Error handlers** (`handler.handle_error(...)` panic should log + continue to the next handler in the chain rather than break it — different policy from the pre-handler segments).
- **Renderers** (`HttpError::to_response` / `RpcError::to_data` / `WsError::to_message` / gRPC `GrpcStatus` rendering — a panic here means the framework can't produce a response at all; fallback is a hardcoded transport-default envelope).

Also still deferred from #82: RPC and WebSocket panic-coverage tests across all wrapped segments (guards, interceptors, and now pipes). The substrate change in this PR is wired into both dispatchers identically; the gap is harness work (TCP wire framing for RPC, WS session lifecycle).

## Test plan

- [x] `cargo build --workspace --all-targets` — clean.
- [x] `cargo test -p integration-tests --test integration` — 198/198.
- [x] `cargo test -p integration-tests --test integration panic_recovery` — 6/6 panic tests pass (handler + observer + guard + interceptor + pipe + ws_handler).
