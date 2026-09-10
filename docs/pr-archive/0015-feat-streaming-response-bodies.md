# #15 — feat: streaming response bodies

Merged 2026-03-26 into `master` from `feat/response-streaming`, commit [`dc40806`](https://github.com/ulo-rs/ulo/commit/dc4080680391939a80dcc73bf6f5d815d6a47dfc).

## Summary

- Replaces the `ToResponse` borrowed trait with a consuming `IntoResponse` trait — controller handlers return `HttpResponse` directly, eliminating the intermediate `Box<dyn ToResponse>` allocation on every request.
- `Body` gains a `Streaming(BoxBody)` variant alongside `Buffered(Bytes)`. Streaming bodies flow to the adapter without allocating the full payload in memory; buffered bodies behave as before.
- The Axum adapter writes `BoxBody` to the wire natively; the Actix adapter collects the stream into `Bytes` (Actix's response API requires a concrete body type).
- The tower-compat bridge no longer buffers Tower response bodies — Tower-transformed bodies (e.g. `CompressionLayer` output) stream through `BoxBody` to the adapter intact. `TowerLayer` is now generic over Tower's response body type `B`.

Request bodies remain fully buffered (`HttpRequest.body: Bytes`). Streaming request bodies through a middleware chain requires careful lifecycle management that isn't warranted for the typical middleware use case.

## Test plan

- [ ] `cargo test -p integration-tests` passes (run single-threaded to avoid port conflicts: `cargo test -p integration-tests -- --test-threads=1`)
- [ ] `cargo build --examples` compiles cleanly
- [ ] `cargo test -p toni --features tower-compat` passes
