# #80 — gRPC enhancer parity: guards, interceptors, error handlers

Merged 2026-05-29 into `master` from `feat/grpc-enhancers`, commit [`0837cfa`](https://github.com/ulo-rs/ulo/commit/0837cfa7d077537034506afa4d4a2c82a80ab613).

## Summary

Brings `#[grpc_methods]` to enhancer parity with `#[rpc_controller]` for guards, interceptors, and error handlers. Each is declared with `#[use_guards(...)]` / `#[use_interceptors(...)]` / `#[use_error_handlers(...)]` at block- or per-method level, resolved against the role registry at bind time, and applied per call without DI lookups on the hot path. A panicking handler now surfaces as `Internal` rather than tearing down the connection.

Per-call dispatch lives in two tonic-free helpers in `toni::grpc_runtime`: `run_grpc_pipeline` walks guards then wraps the user delegation in the interceptor chain, and `run_grpc_error_chain` walks service- + method-level error handlers in reverse registration order on either a user-returned `Err(Status)` or a `PanicRecovered` event. `catch_handler_panic` re-types the panic payload so observers see the typed framework event rather than a stringified status.

## Components

**Core (`toni`)**
- `context::GrpcContext` — method path, ASCII metadata, peer addr, error-only response slot; pipes deliberately not supported (see below).
- `grpc_status::{GrpcCode, GrpcStatus}` — transport-neutral mirror of `tonic::Status` so the chain logic stays tonic-free.
- `ProviderRole::GrpcGuard` / `::GrpcInterceptor` / `::GrpcErrorHandler` + matching role-registry slots + container globals.
- `ResolvedGrpcEnhancers` (per-service bundle including observers); `GrpcServiceTrait::register_with(registrar, enhancers)` takes it as a second arg.
- `injector::GrpcServiceResolver` parallels `RpcControllerResolver` — walks token getters, rejects request-scoped factories at startup.
- `grpc_runtime::{run_grpc_pipeline, run_grpc_error_chain, run_grpc_guards, catch_handler_panic}`.

**Adapter (`toni-grpc`)**
- `GrpcAdapter::bind` signature updated to receive `(Arc<service>, Arc<bundle>)` pairs and forward both into `register_with`.

**Macros (`toni-macros`)**
- `EnhancerKind::GrpcGuard` / `::GrpcInterceptor` + `ErrorHandlerKind::Grpc` spec entries.
- `#[guard(grpc)]` / `#[interceptor(grpc)]` / `#[error_handler(grpc)]` parsing and `Guard<GrpcContext>` / etc. typed-impl-head detection.
- `#[grpc_methods]` parses block- and per-method enhancer attrs, emits token-getter implementations, and generates a hidden `__<Self>Enhanced` wrapper that holds `Arc<UserType>` + `Arc<ResolvedGrpcEnhancers>`, runs the chain, then delegates via UFCS — `<UserType as ProtoTrait>::method(&inner, req).await`. UFCS keeps the user's original impl block unmodified so `Self::SomeStream` associated types, `self.<field>` accesses, and inherent helper calls all resolve in the user's context.

**Tests (`integration-tests`)**
- 9 new tests in `rpc_grpc_macros.rs` covering the three layers: guards (accept / reject / method-stacks), interceptors (around-ordering / short-circuit / method-stacks), error handlers (claim+remap / pass-through / panic recovery).
- 193/193 integration tests pass.

## Deferred to follow-up

- **Pipes** (`#[use_pipes]` on `#[grpc_methods]`): a typed proto payload can't sit in a non-generic `GrpcContext`, and a metadata-only pipe role adds no expressive power over interceptors. Revisit only if a concrete user case shows up.
- gRPC docs (README + crate-level doc + one example wiring DI + a guard + a `.proto` end-to-end) — separate PR.

## Test plan

- [ ] CI green on the workspace.
- [ ] `cargo test -p integration-tests --test integration` (193 tests).
- [ ] Spot-check that the existing 5 `rpc_grpc_macros` tests still pass alongside the 9 new ones — confirms the runtime + macro rewrite preserves pre-PR behavior on services that don't declare any enhancers.
