# #224 — Show gRPC reflection and health as services, not knobs

Merged 2026-09-01 into `master` from `docs/grpc-reflection`, commit [`fb94e15`](https://github.com/ulo-rs/ulo/commit/fb94e15bea5143d1eebd86ca14eabf9e0874fd68).

A server `grpcurl` can explore without a local `.proto` needs nothing added: `tonic-reflection` produces a tonic service, and `GrpcAdapter::add_service` accepts tonic services. Nothing said so, which left reflection reading as unsupported.

```rust
// build.rs
tonic_prost_build::configure()
    .file_descriptor_set_path(&descriptor)
    .compile_protos(&["proto/orders.proto"], &["proto"])?;

// main.rs
let reflection = tonic_reflection::server::Builder::configure()
    .register_encoded_file_descriptor_set(DESCRIPTOR)
    .build_v1()?;
let adapter = GrpcAdapter::new(addr).add_service(reflection);
```

`grpcurl -plaintext 127.0.0.1:50051 list` then answers with the services `#[grpc_methods]` registered.

## Why no `with_reflection`

ADR-0034's test: the framework owns no lifetime here. A knob would also take `tonic-reflection` as a dependency of `toni-grpc` — a version of someone else's optional crate, carried to save four lines — and would have to pick which schema versions to expose by default. Reflection publishes an API to anyone who asks, and newer tooling speaks `grpc.reflection.v1` while older speaks `v1alpha`; that belongs in the deployment's code where it can be read, not behind a boolean.

`tonic-health` is the same shape and already worked this way in `rpc_grpc.rs`.

## What this adds

- The crate doc's "Reflection and health" section, with the `build.rs` half, which is the part easiest to get wrong.
- `grpc_reflection.rs`, which pins the property worth pinning: a client holding no `.proto` is told about `toni_test.orders.Orders`, so the manually-added service and the DI-discovered one reached the same route set.
