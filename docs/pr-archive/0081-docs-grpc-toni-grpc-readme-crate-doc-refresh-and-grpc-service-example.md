# #81 — docs(grpc): toni-grpc README, crate doc refresh, and grpc_service example

Merged 2026-05-30 into `feat/grpc-enhancers` from `docs/grpc-readme-and-example`, commit [`ab710a1`](https://github.com/ulo-rs/ulo/commit/ab710a19e3ddc56a8e346e18e6a6de21ba15413c).

## Summary

Adds the docs surface for `toni-grpc` so a reader can wire up a gRPC service end-to-end without reading the framework source. Stacks on top of #80 (gRPC enhancer parity) and uses the enhancer attrs introduced there.

## Components

**`toni-grpc/README.md`** (new)
- Quickstart with a minimal `#[grpc_service]` + `#[grpc_methods]` wire-up.
- Per-section coverage of guards / interceptors / error handlers, including the deliberate non-support of pipes (the proto payload is method-typed and can't fit a non-generic `GrpcContext`).
- Streaming, graceful shutdown (`with_drain_timeout`), tracing-span fields, and mixing DI services with manually-`add_service`'d ones.

**`toni-grpc/src/lib.rs`**
- Crate doc rewritten to mirror the README's shape at a smaller scale — minimal example, drain timeout, tracing, mixing modes — so the rustdoc landing carries enough to get started.

**`examples/grpc_service.rs`** (new)
- One runnable file demonstrating every enhancer hook the README mentions: DI'd counter, `Guard<GrpcContext>` reading metadata, `Interceptor<GrpcContext>` logging around the call, `ErrorHandler<GrpcContext, GrpcStatus>` remapping a domain-tagged status.
- The header comment includes copy-paste `grpcurl` invocations for the authorised, unauthorised, and error-remap paths so a reader can verify each hook fires.

## Test plan

- [x] `cargo build --example grpc_service` — clean.
- [x] `cargo run --example grpc_service` — binds 127.0.0.1:50051 and logs "gRPC listening".
- [x] `cargo doc --no-deps -p toni-grpc` — clean, no broken intra-doc links.
- [x] `cargo test -p integration-tests --test integration` — 193/193 still pass.

## Notes for the reviewer

- Base is `feat/grpc-enhancers` (#80) so the example can use `#[use_guards]` / `#[use_interceptors]` / `#[use_error_handlers]`. Rebase onto `master` after #80 merges.
- The README's link to `monterxto/toni-rs` for the main repo matches the existing convention in `toni-async-graphql/README.md` and the rest of the integration crates.
