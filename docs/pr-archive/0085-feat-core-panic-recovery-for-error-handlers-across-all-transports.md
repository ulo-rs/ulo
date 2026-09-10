# #85 — feat(core): panic recovery for error handlers across all transports

Merged 2026-05-30 into `master` from `feat/panic-coverage-error-handlers`, commit [`a4672f2`](https://github.com/ulo-rs/ulo/commit/a4672f234cf2b26d4e552de8577a590fdc8e0dd0).

## Summary

Closes the error-handler segment of the pipeline-panic coverage work. Until now a panic from a registered chain handler's `handle_error` propagated up the dispatcher, killing the rest of the chain and preventing the fallback envelope from rendering — the original error the chain was supposed to remap got lost. A panicking chain handler now fans `PanicRecovered { during: ErrorHandler }` to observers and is treated as a `None` claim so subsequent handlers (and the fallback) still get their turn.

## Policy difference vs. pre-handler segments

The guards / interceptors / pipes shipped earlier render a wire-level Internal envelope on panic because the dispatcher can't trust the context after a partial run. Error handlers run when there's *already* an error to recover from, so killing the chain on the first panic would starve every later handler of its chance to claim AND lose the original error. The log-and-continue policy preserves the chain semantics: any combination of handlers can fail (return `None` or panic) and the original error still reaches the fallback rendering.

## Components

**Dispatcher wrap-ups** (HTTP / RPC / WS / gRPC)
- Nine `handler.handle_error(...)` call sites — four in `InstanceWrapper`, two each in `RpcControllerWrapper` and `GatewayWrapper`, one in `grpc_runtime::run_grpc_error_chain` — each now goes through `catch_async(PipelineSegment::ErrorHandler, ...)`. On `Err(panic_event)`, the dispatcher fans the typed event to observers and continues to the next handler.
- HTTP, RPC, and WS factor the pattern into a `try_chain_handler` helper next to `fan_out_observers` so the loop bodies stay one line. gRPC inlines because it has a single site and is a free-function module rather than an impl block.

**Tests**
- HTTP: one new test in `panic_recovery.rs` — `panicking_error_handler_continues_chain` registers a single always-panicking chain handler against a panicking user handler, asserts the fallback `HttpError::to_response` still produces the 500 envelope with the original panic message, and asserts the observer sees both events in order (`HandlerBody` first, then `ErrorHandler`).
- 199/199 integration tests pass.

## Deferred to follow-up

One pipeline segment still propagates panics unprotected:
- **Renderers** (`HttpError::to_response` / `RpcError::to_data` / `WsError::to_message` / gRPC `GrpcStatus` rendering — a panic here means the framework can't produce a response at all; fallback is a hardcoded transport-default envelope written directly at the call site).

Also still deferred: RPC, WebSocket, and gRPC error-handler panic tests, alongside the existing RPC + WS guard / interceptor / pipe test gap from the earlier panic-coverage PRs. The substrate change in this PR is wired into every dispatcher identically; the gap is harness work.

## Test plan

- [x] `cargo build --workspace --all-targets` — clean.
- [x] `cargo test -p integration-tests --test integration` — 199/199.
- [x] `cargo test -p integration-tests --test integration panic_recovery` — 7/7 (handler + observer + guard + interceptor + pipe + error_handler + ws_handler).
