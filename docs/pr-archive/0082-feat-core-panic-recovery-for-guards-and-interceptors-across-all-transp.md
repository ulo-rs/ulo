# #82 — feat(core): panic recovery for guards and interceptors across all transports

Merged 2026-05-30 into `master` from `feat/panic-coverage-pipeline`, commit [`d8e43fe`](https://github.com/ulo-rs/ulo/commit/d8e43fe1c02511fdf4f985e2db874ffd0cb0796c).

## Summary

A panic from a guard's `can_activate` or an interceptor's `intercept` tore down the request the same way handler panics used to before the error layer landed — only `PipelineSegment::HandlerBody` was inside `AssertUnwindSafe(...).catch_unwind()`. The `Guard` and `Middleware` segment variants existed but had no producer; observers and the chain never saw a `PanicRecovered` tagged with either. This PR wraps both segments across HTTP / RPC / WebSocket / gRPC so a panicking guard or interceptor surfaces as a structured wire response and observers see the typed event with the segment that caused it.

## Components

**`toni::panic_recovery`** (new module)
- `catch_async<Fut, T>(segment, fut) -> Result<T, PanicRecovered>` and `catch_sync<F, T>(segment, f) -> Result<T, PanicRecovered>` factor the `AssertUnwindSafe(...).catch_unwind()` pattern so each dispatcher tags its own segment without duplicating the boilerplate.
- `grpc_runtime::catch_handler_panic` is now a thin wrapper over `catch_async(HandlerBody, ...)` — the macro keeps a stable transport-specific entry point.

**Dispatcher wrap-ups** (HTTP / RPC / WS / gRPC)
- Each guard's `can_activate` call: panic → observers see `PanicRecovered { during: Guard }`, wire response is the transport's rejection envelope (HTTP 403 / RPC `Forbidden` / WS `AuthFailed` / gRPC `PermissionDenied`) carrying the panic message.
- Each interceptor's `intercept` call (including the recursive chain links): panic → observers see `PanicRecovered { during: Middleware }`, the chain's error handlers get first claim before rendering falls back to the transport's default Internal envelope.

**Tests**
- HTTP: two new tests in `panic_recovery.rs` — panicking guard renders 500 with `PipelineSegment::Guard` on the observer event; panicking interceptor renders 500 with `PipelineSegment::Middleware`.
- gRPC: two new tests in `rpc_grpc_macros.rs` — panicking guard surfaces as `PermissionDenied`, panicking interceptor surfaces as `Internal`. A second call on the same channel proves the server stays up.
- 197/197 integration tests pass.

## Deferred to follow-up

Three pipeline segments still propagate panics unprotected:
- **Pipes** (`pipe.process(ctx)` is sync — needs `catch_sync` at each dispatcher's pipe loop).
- **Error handlers** (`handler.handle_error(...)` panic should log + continue to the next handler in the chain rather than break it).
- **Renderers** (`HttpError::to_response` / `RpcError::to_data` / `WsError::to_message` / gRPC `GrpcStatus` rendering — a panic here means the framework can't produce a response at all; fallback is a hardcoded transport-default envelope).

Also deferred: explicit RPC and WebSocket panic-coverage tests. The substrate change in this PR is wired into both dispatchers identically; the gap is just test harnesses (TCP wire framing for RPC, WS session lifecycle).

## Test plan

- [x] `cargo build --workspace --all-targets` — clean.
- [x] `cargo test -p integration-tests --test integration` — 197/197.
- [x] Spot-check that the two new HTTP panic tests + two new gRPC panic tests verify both the wire envelope and the observer `PipelineSegment`.
