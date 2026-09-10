# #222 — Read the caller's deadline off grpc-timeout

Merged 2026-08-31 into `master` from `feat/grpc-deadline`, commit [`f971835`](https://github.com/ulo-rs/ulo/commit/f971835ed3fea107f99cedf7f7c41da7d96c73a6).

`HandlerContext::deadline` answered `None` on every context, and its own doc comment named gRPC's `grpc-timeout` as the obvious first producer. gRPC is the only transport whose wire carries how long the caller intends to wait, so a handler had to guess at a budget the caller had already stated.

```rust
async fn create(&self, request: Request<CreateOrder>) -> Result<Response<Order>, Status> {
    let ctx = GrpcContext::of(request.extensions()).unwrap();
    match ctx.deadline() {
        Some(by) if by.saturating_duration_since(Instant::now()) < MIN_BUDGET => {
            return Err(Status::deadline_exceeded("not enough time to start"));
        }
        _ => {}
    }
    // …
}
```

## It reports, tonic enforces

tonic already ends an overrunning call with `DeadlineExceeded`, taking the shorter of the caller's header and any server-configured timeout (`transport/service/grpc_timeout.rs`). Nothing about that changes. What was missing was any way for a guard or handler to know how long it had before it happens — which is what decides whether to start work at all.

## No generated code changes

`grpc-timeout` arrives as ordinary metadata, and `#[grpc_methods]` already collects the ASCII metadata into the map it hands `GrpcContext::new`. The context derives the deadline there, once, so every reader sees the same instant rather than each recomputing from a clock that has moved.

The parser is toni's own: tonic's is `pub(crate)`. A value the spec does not define reads as absent rather than refusing the call, since the caller may not know it sent it and tonic refuses the malformed ones it enforces itself.

## Tests

Six unit tests over the parser — every unit the spec defines, and six shapes it does not. Two integration tests: a client that sets a timeout is seen by the handler as a deadline inside the budget it sent, and one that sets none has none.
