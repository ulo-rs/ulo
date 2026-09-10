# #83 — docs(grpc): toni-grpc README, crate doc refresh, and grpc_service example

Merged 2026-05-30 into `master` from `docs/grpc-readme-and-example-v2`, commit [`82b2b8a`](https://github.com/ulo-rs/ulo/commit/82b2b8a3fa87dd3d8af856cf977d54076b39f053).

## Summary

Re-routes the docs work from #81 onto `master`. The original PR was based on `feat/grpc-enhancers` and merged against that branch, so the commits never reached `master` after #80's merge resolved differently. These two commits are the same content, cherry-picked onto current `master`.

## Components

**`toni-grpc/README.md`** (new)
- Quickstart with a minimal `#[grpc_service]` + `#[grpc_methods]` wire-up.
- Per-section coverage of guards / interceptors / error handlers, including the deliberate non-support of pipes (the proto payload is method-typed and can't fit a non-generic `GrpcContext`).
- Streaming, graceful shutdown (`with_drain_timeout`), tracing-span fields, and mixing DI services with manually-`add_service`'d ones.

**`toni-grpc/src/lib.rs`**
- Crate doc rewritten to mirror the README's shape at a smaller scale.

**`examples/grpc_service.rs`** (new)
- One runnable file demonstrating every enhancer hook the README mentions: DI'd counter, `Guard<GrpcContext>` reading metadata, `Interceptor<GrpcContext>` logging around the call, `ErrorHandler<GrpcContext, GrpcStatus>` remapping a domain-tagged status.
- The header comment includes copy-paste `grpcurl` invocations for the authorised, unauthorised, and error-remap paths.

## Test plan

- [x] `cargo build --example grpc_service` — clean.
- [x] `cargo run --example grpc_service` — binds 127.0.0.1:50051.
- [x] `cargo doc --no-deps -p toni-grpc` — clean.
- [x] `cargo test -p integration-tests --test integration` — passes.
