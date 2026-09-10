# #70 — feat(rpc): GrpcAdapter trait + uniform ServerLifecycle orchestration

Merged 2026-05-02 into `master` from `feat/grpc-adapter-v1`, commit [`45b994f`](https://github.com/ulo-rs/ulo/commit/45b994fef508af511caff7e92cfbd2a4c394d583).

PR 1 of 5 for the gRPC transport. Introduces the framework seam, refactors the existing adapter orchestration onto a uniform lifecycle protocol, and ships a stub `toni-grpc` crate that wraps `tonic::transport::Server`.

## Why this shape

gRPC is contract-first (services declared in `.proto`), supports four call modes (unary + three streaming), and dispatches via tonic-generated trait impls. None of that fits the pattern-string + JSON-data + unary contract `RpcAdapter` encodes for TCP/UDP/NATS — so `GrpcAdapter` is a separate trait, not a variant.

The shared concept — bind, serve, drain, surface a local address — moves to a new internal `ServerLifecycle` trait that every adapter kind's lifecycle handle implements. The framework's startup/shutdown code is now one loop over `Vec<Box<dyn ServerLifecycle>>` instead of four per-transport branches; future Kafka/MQTT/RabbitMQ adapters plug in by adding a new `*LifecycleHandle: ServerLifecycle` plus a typed `use_*_adapter()` method.

Public adapter traits (`HttpAdapter`, `WebSocketAdapter`, `RpcAdapter`) keep their existing methods so the 8 downstream adapter crates don't change. Slimming those traits is a follow-up; this PR's blast radius is just the orchestration layer plus the new gRPC seam.

## What's in the PR

- `ServerLifecycle` trait + `*LifecycleHandle` types in `toni/src/adapter/`.
- `GrpcAdapter` trait + `ErasedGrpcAdapter` facade.
- `ToniApplication` now stores `Vec<Box<dyn ServerLifecycle>>`; `bind()`/`close_adapters()` are uniform loops.
- New `use_grpc_adapter()` registration. `BoundAdapters.grpc: Option<SocketAddr>` populated after bind.
- New crate `toni-grpc` (0.1.0) wrapping `tonic::transport::Server`. `GrpcAdapter::new(addr).add_service(svc)` builder.
- Smoke test using `tonic-health` to verify a real gRPC round-trip through the seam.

## Drive-by

Dropped the `#[path = "adapter/http_adapter.rs"]` artifact in `lib.rs` — `http_adapter` lives cleanly inside `adapter/` like every other adapter module.

## Out of scope (follow-up PRs)

- `#[grpc_service]` / `#[grpc_methods]` macros (PR 2)
- Per-request tracing layer (PR 3)
- Streaming support (PR 4)
- Example + docs (PR 5)
- Slimming public adapter traits to drop lifecycle methods now that handles own them (separate refactor)

## Tests

- New `rpc_grpc::grpc_adapter_seam_round_trip_and_shuts_down`: registers tonic-health, makes a real gRPC `Health/Check`, asserts `SERVING`, drains.
- All 162 existing integration tests still pass — the lifecycle refactor preserves HTTP/WS/RPC/broadcast/tower-compat/graceful-shutdown behavior unchanged.
