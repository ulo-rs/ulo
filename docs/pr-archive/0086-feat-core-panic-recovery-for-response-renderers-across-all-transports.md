# #86 — feat(core): panic recovery for response renderers across all transports

Merged 2026-05-30 into `master` from `feat/panic-coverage-renderers`, commit [`b51f599`](https://github.com/ulo-rs/ulo/commit/b51f5992572a6aa231df6a11759c283727128044).

## Summary

Closes the last segment of the pipeline-panic coverage work. The renderers — `HttpError::to_response`, the free function `http_error::render_error`, `RpcError::to_data`, `WsError::to_message` — were the only remaining call sites that could panic mid-dispatch and leave the framework with no safe value to put on the wire. A `toni::Error` implementation whose `message` or `details` panics would crash the dispatcher even after every other segment was caught (guards, interceptors, pipes, error handlers all landed in earlier PRs).

## Policy

A renderer panic fans `PanicRecovered { during: ResponseRendering }` to observers, then substitutes a hardcoded minimal envelope. The fallback is built from simple constructors that don't render any user-supplied data — `Body::text("Internal Server Error")` for HTTP, `RpcData::text("Internal Server Error")` for RPC, `WsMessage::text("Internal Server Error")` for WS. Recursive panic during the fallback path is structurally impossible.

## Components

**Dispatcher wrap-ups** (HTTP / RPC / WS)
- Per-transport `safe_render` helpers next to the existing `try_chain_handler` ones drive the renderer through `catch_sync(PipelineSegment::ResponseRendering, ...)`. The chain-exhaustion sites become one-liner calls.
- The `unwrap_or_else` pattern in HTTP `handle_framework_event` becomes an explicit `if let / else` so the fallback renders through `safe_render` instead.
- Per-transport `fallback_*` helpers return the hardcoded envelopes.

**gRPC** — no wrap needed. The macro emits `tonic::Code::from_i32(...)` + `tonic::Status::new(...)` to convert `GrpcStatus`, both of which are infallible. There's no render call site that can panic on user data.

**Tests**
- HTTP: one new test in `panic_recovery.rs` — `panicking_renderer_falls_back_to_safe_envelope` installs a domain error whose `message()` panics, the handler returns it, `HttpError::to_response` calls `message()` and panics. Assertions: 500 status, body is the hardcoded `"Internal Server Error"`, observer fires twice (original error + renderer panic event) with the final captured segment being `ResponseRendering`.
- 200/200 integration tests pass.

## Item #4 status

With this PR, all five pipeline segments are covered: HandlerBody (pre-existing), Guard + Middleware (#82), Pipe (#84), ErrorHandler (#85), ResponseRendering (this PR). Any panic in framework-invoked user code now surfaces as a `PanicRecovered` event tagged with the matching `PipelineSegment` and routes through the observer + fallback pipeline.

## Deferred to follow-up

RPC, WebSocket, and gRPC panic-coverage tests across every segment remain — same gap that's been carried forward since #82. The substrate change in every PR is wired into each dispatcher identically; what's missing is the harness work (TCP wire framing for RPC, WS session lifecycle for the gateway tests). That's the obvious next cleanup before any new transport candidate (Kafka / MQTT / RabbitMQ / Redis Pub-Sub).

## Test plan

- [x] `cargo build --workspace --all-targets` — clean.
- [x] `cargo test -p integration-tests --test integration` — 200/200.
- [x] `cargo test -p integration-tests --test integration panic_recovery` — 8/8 (handler + observer + guard + interceptor + pipe + error_handler + renderer + ws_handler).
