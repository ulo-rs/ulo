# 29 grpc-service

A gRPC service from a `.proto`: `ulo-build` generates the messages, a client and one marker type per
method; handlers bind to the markers with `#[ulo_grpc::method]`. One unary method, one
server-streaming method, a guard that reads the call's metadata, and a domain error that reaches
the caller as a status code plus `google.rpc` details.

```
cargo run -p ex-29-grpc-service
```

The server listens on `PORT` (default 50051) and prints `listening on http://ADDR`; Ctrl-C closes it.
Any gRPC client reaches it, for example `grpcurl -plaintext -H 'authorization: Bearer open'
-d '{"id":"a1"}' -import-path examples/29-grpc-service/proto -proto catalog.proto
127.0.0.1:50051 catalog.v1.Catalog/GetItem`.

Read first:

- `proto/catalog.proto` and `build.rs`: the whole build step is one `ulo_build::configure().compile(..)`
  call. `ulo-build` vendors `protoc`, so none is installed; a `PROTOC` variable wins when set.
- `src/lib.rs`: `TokenGuard` refuses with `CallError::unauthorized`, which gRPC renders as
  UNAUTHENTICATED (`Ok(false)` would be PERMISSION_DENIED). `ItemError` implements `Classify`: its
  kind picks the status code and its `details()` travel in `grpc-status-details-bin`.
- `tests/service.rs`: replies, the guard on both methods, and the details decoded from the trailer.
- `tests/binary.rs`: the compiled binary called through the generated client, then SIGTERM.

Toolchain: the crates here build on Rust 1.88, the workspace floor (`cargo +1.88 check -p ex-29-grpc-service`).

Next: [30 grpc-client](../30-grpc-client).
