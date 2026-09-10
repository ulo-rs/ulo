# #71 — feat(grpc): #[grpc_service] + #[grpc_methods] macros with DI auto-discovery

Merged 2026-05-02 into `master` from `feat/grpc-macros`, commit [`07e73ef`](https://github.com/ulo-rs/ulo/commit/07e73ef4c40f2d0c36edad99b5b383fc2f5b281d).

PR 2 of 5 for the gRPC transport. Brings gRPC handlers to ergonomic parity with `#[rpc_controller]`: declare a service struct, write the proto-trait impl, register the service in your module's `providers: [...]`, and the framework discovers + registers it with the gRPC adapter at bind time. The user never types `*Server::new(handler)` and never calls `adapter.add_service()`.

## What's in the box

**Two macros** (`toni-macros`):

- `#[grpc_service]` — sits on the struct declaration plus its inherent impl. Same mechanics as `#[rpc_controller]`: `#[inject]`-aware fields, `new()` constructor, registers a `ProviderRole::GrpcService` so the container's role registry surfaces it at bind time.
- `#[grpc_methods]` — sits on the proto-trait impl block. The user's impl passes through unchanged; the macro just emits an additional `impl GrpcServiceTrait` whose `register_with` body wraps `self` in the inferred `*Server` and adds it to a `RoutesBuilder`. The wrapper type is inferred from the proto trait name (`Orders` → `OrdersServer` in the same parent path); override with `#[grpc_methods(server = path::to::OrdersServer)]` when the convention doesn't match.

**Framework plumbing** (`toni`):

- `GrpcServiceTrait` in `toni::adapter` — `token()` + `register_with(&mut dyn Any)`. The `dyn Any` indirection keeps tonic types out of toni core; only the macro-generated impl and the toni-grpc adapter mention tonic.
- `ProviderRole::GrpcService` variant + `RoleRegistry::grpc_services` map.
- `ToniApplication::bind` collects discovered services from the registry and threads them through `GrpcAdapter::bind(services)`.

**Adapter** (`toni-grpc`):

- `GrpcAdapter` switches its accumulator from `Routes` to `RoutesBuilder`. Manual `add_service` calls and macro-discovered services land in the same builder before the listener spins up.

## Test

`integration-tests/tests/integration/rpc_grpc_macros.rs` defines an `OrdersCounter` provider, an `OrdersGrpcService` that injects it, and a real `Orders.Create` proto method. The test calls the service over a real gRPC client and asserts the response uses the injected counter — proving DI flows through the full pipeline: container → RoleRegistry → bind → adapter → tonic Server → handler.

`tonic-prost-build` (with `protoc-bin-vendored` so CI doesn't need a system `protoc`) compiles the tiny `orders.proto` at build time.

## Out of scope

- Per-request tracing layer (next PR).
- Streaming RPCs.
- Guard / interceptor reuse on gRPC handlers.
