# #242 — Carry the domain error on the gRPC status

Merged 2026-09-05 into `master` from `feat/grpc-status-carries-its-error`, commit [`b9ac65a`](https://github.com/ulo-rs/ulo/commit/b9ac65ad50e8d26bec2d342b6068d2b6d073f50c).

A `GrpcStatus` built from a domain error keeps that error, and the generated method attaches it to the answer through `tonic::Status`'s own source slot.

**The error reaches the chain without the extension bag.** `stash_failure` and `take_failure` are gone. The error travels with the status it belongs to rather than beside it, so it is no longer read back by a rule — the next failure in this execution is mine — and a call dispatched without a toni context keeps its type too. The slot is typed `dyn std::error::Error`, which drops the `Send + Sync` the chain needs, so the error is wrapped in a concrete `GrpcFailure` that a downcast recovers the bound from. Nothing goes to the wire: the slot is in-process.

**A handler names any gRPC code.** `GrpcStatus` implements `toni::Error`, so a handler returns one directly:

```rust
Err(GrpcStatus::new(GrpcCode::FailedPrecondition, "the booking window is closed"))
```

`grpc_code` maps eleven kinds onto the canonical codes; `FailedPrecondition`, `OutOfRange`, `AlreadyExists`, `DataLoss` and `Cancelled` are outside it, and reaching one meant registering a chain handler to claim the error and answer with the status. `GrpcStatus::of` answers a status as itself rather than re-deriving one, so a named code is not flattened to `Internal`. `caused_by` keeps a domain error on a hand-named status, so the code goes to the wire and `#[catch(WindowClosed)]` still matches the type.

- **Breaking** — the blanket `impl<E: toni::Error> From<E> for GrpcStatus` is removed. With `GrpcStatus` as a `toni::Error` it would give `From<GrpcStatus> for GrpcStatus`, colliding with the reflexive impl in the standard library. `GrpcStatus::of` is the owned conversion and `from_error` the borrowed one. `GrpcStatus` gains a private field, so a struct literal no longer builds one.
- **`error_kind(code)`** is the inverse of `grpc_code` where the table has an answer and `Internal` where it does not, which is what a `GrpcStatus` renders as on a transport that reads kinds.
- **Docs** — ADR-0039, which supersedes ADR-0037's mechanism.
