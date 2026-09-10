# #141 — Let gRPC handlers read what their guards attached

Merged 2026-08-14 into `master` from `feat/grpc-extension-reach`, commit [`fdc850f`](https://github.com/ulo-rs/ulo/commit/fdc850fa6372e28c20bae427e898e2c65cc3ef16).

gRPC handlers can now read the request's extension bag, which completes the enhancer-to-handler channel across all four transports.

It was the one transport still missing it. A gRPC handler receives the tonic request and never `GrpcContext` — `#[grpc_methods]` builds the context inside the generated wrapper, runs the pipeline with it, and delegates to a handler whose signature the tonic trait dictates. So a guard could authenticate a caller and the handler had no way to see the result.

**The mechanism.** The bag rides the tonic request. tonic's `Request::extensions()` hands back `http::Extensions` — the same type an HTTP request carries — so `Extensions::adopt` reads it back on either side and gRPC needs no API of its own:

```rust
async fn create(&self, request: Request<CreateOrder>) -> Result<Response<Order>, Status> {
    let principal = Extensions::adopt(request.extensions()).get::<Principal>();
    // ...
}
```

A handle travels rather than a copy, so the guards that run after it is attached write into the bag the handler reads. The handler signature is untouched.

**Tests.** A guard attaches a principal and the handler returns what it read; removing the threading makes it read `ABSENT`. The fixture takes its own service and port, matching the guard fixtures beside it so the duplicate proto-trait impls never share a running server.

**Docs.** The rustdoc on `Guard` now names how the handler reads the bag on each transport, and `Extensions::adopt` carries the gRPC example — it is a user-facing call there rather than only a framework one.
