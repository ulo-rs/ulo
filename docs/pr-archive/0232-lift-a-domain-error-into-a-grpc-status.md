# #232 — Lift a domain error into a gRPC status

Merged 2026-09-02 into `master` from `feat/grpc-error-lift`, commit [`7d41a09`](https://github.com/ulo-rs/ulo/commit/7d41a09b7126d7fe9397945f366fd947462263af).

A `toni::Error` answers a gRPC call with the code its `kind()` means. On the other three transports `?` does this: the handler returns `Err(OutOfStock)` and the dispatcher renders 409, or the `Conflict` envelope. A gRPC handler's signature belongs to tonic, so it returned whatever code the author picked by hand, and the same `NotFound` could answer two different ways in one codebase.

- **Core** — `grpc_code(kind)` is the canonical HTTP-to-gRPC table, alongside `http_status(kind)` which has always been there, plus `impl<E: toni::Error> From<E> for GrpcStatus`. `Conflict` maps to `Aborted`, the table's answer for 409; a service meaning "already exists" says so explicitly.
- **toni-grpc** — `to_status(e)` is the last hop into `tonic::Status`, which core cannot make: the orphan rule forbids converting into a foreign type. A handler writes `.map_err(toni_grpc::to_status)?`, or gives its own error type a one-line `From` impl and keeps bare `?`.
- **Example** — `grpc_service.rs` gains a domain error and answers `ABORTED` for it, with the `grpcurl` line that shows it.
- **Tests** — the kind table, the conversion, and one call over the wire reading the code back off tonic.

What crosses here is the kind, not the identity: the handler answers with a status, so a `#[catch(OutOfStock)]` on this transport does not match, where the same catcher works on HTTP, RPC and WebSocket. Carrying the type as well is deferred: it needs a place to put the error that `tonic::Status` does not have.
