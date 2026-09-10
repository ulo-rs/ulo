# #234 — Keep a gRPC handler's error type for the error chain

Merged 2026-09-02 into `master` from `feat/grpc-chain-sees-domain-errors`, commit [`3894103`](https://github.com/ulo-rs/ulo/commit/3894103c41c3b689d694b940cd08c3596325d82e).

A `#[catch(OrderError)]` handler matches on gRPC. It already matches on the other three transports, where the handler's error type is toni's own and the value rides the return inside an `AppError` variant. tonic fixes the gRPC signature to `Status`, which holds a code and a message and has nowhere to keep the error, so the chain was handed the flattened status and a catcher could only match what survived — the framework's own example matched a substring for want of a type.

The error now travels beside the answer when it cannot travel inside it:

```rust
let ctx = GrpcContext::of(request.extensions()).expect("a toni-dispatched call");
let id = reserve(&req.item).fail_with(&ctx)?;      // or: Err(ctx.fail(OutOfStock { item }))
```

`fail` parks the domain error on the execution and answers with the status its `kind()` maps to. The `#[grpc_methods]` wrapper takes it on the way to the chain and hands that over in place of the status.

- **Core** — `stash_failure` / `take_failure` on the execution's extensions, plus `GrpcStatus::from_error` for reading a status off an error the framework does not own.
- **toni-grpc** — `GrpcFail::fail(e)` and `FailWith::fail_with(&ctx)`, the `?`-shaped form.
- **Example** — `grpc_service.rs` drops `error.to_string().contains("invalid-qty")` for `error.downcast_ref::<InvalidQty>()`.
- **Tests** — a catcher claims the domain type and rewrites the answer; the same failure with nothing registered still renders the code its kind maps to.

`to_status` stays for the paths with no context to hand over: an `impl From<OrderError> for tonic::Status` in the caller's own crate is what buys bare `?`, and an impl cannot reach an execution. The rule is inside a handler, `fail`; without a context, `to_status`, and the chain sees a status.

Nothing changes on the wire — the status a caller receives is identical either way. What changes is which handlers on the server can act on it.

Implements ADR-0037. Depends on #232.
