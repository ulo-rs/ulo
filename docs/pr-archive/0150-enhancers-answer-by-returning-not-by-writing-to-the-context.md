# #150 — Enhancers answer by returning, not by writing to the context

Merged 2026-08-16 into `master` from `feat/interceptors-return-their-answer`, commit [`9c42eef`](https://github.com/ulo-rs/ulo/commit/9c42eefef7d9959e0d08e4d9c46daaf49fa6c734).

`Interceptor::intercept` and `Pipe::process` both returned `()`, so neither could answer a request except by mutating a response slot on the context — a pipe pairing it with `abort()` to stop the chain. A guard's response, if it left one, was read back and preferred over the canonical rejection envelope. Middleware and error handlers already answered by returning, which left the slot serving three participants out of five with no rule saying which of the two ways won.

Returning is now the only way.

```rust
pub trait Interceptor<C: ?Sized + HandlerContext, R>: Send + Sync {
    async fn intercept(&self, ctx: &mut C, next: Box<dyn InterceptorNext<C, R>>) -> R;
}

pub trait Pipe<C: ?Sized + HandlerContext, R>: Send + Sync {
    fn process(&self, data: &mut C) -> Option<R>;
}
```

`set_response`, `response`, `response_mut`, `take_response` and `into_response` are gone from all four contexts, and the four dispatchers return their answer instead of carrying it to the exit through a slot. Guards keep `bool`.

## What this buys beyond uniformity

The slot's type on WebSocket could not hold a stream. A gateway handler returning one reached the dispatcher through an `Arc<Mutex<Option<BoxStream>>>` threaded beside the context through five functions, for no reason other than the slot's shape. Answering with `WsHandlerResult` removes that channel and collapses three `ExecutionResult::Ok` arms into one.

Two error branches — "request aborted by interceptor without response" on RPC and its WebSocket twin — are unreachable rather than ported. An interceptor that must return `R` cannot stop the chain with nothing to send, so the type removes the condition instead of the code that checked for it.

## Components

- **Traits** — `Interceptor<C, R>`, `InterceptorNext<C, R>`, `Pipe<C, R>`. `R` is concrete per transport, as `ErrorHandler<C, R>` already was.
- **Answer aliases** — `RpcHandlerResult` and `GrpcHandlerResult` join the existing `WsHandlerResult`, so an impl header reads `Interceptor<RpcContext, RpcHandlerResult>` rather than spelling the `Result<Option<RpcData>, RpcError>` out.
- **Dispatchers** — HTTP, RPC, WebSocket and gRPC, including the error-handler and panic-recovery paths.
- **Macros** — the enhancer role-detection emits the two-parameter trait paths.
- **ADR 0016** — records the execution-context decision this is the first step of.

## A distinction worth keeping in review

A pipe that answers returns `Some`. A pipe that aborts without one produces a transport-level error, not a success frame carrying an error envelope. That separates a rejected request from a user error, and it is the one behaviour the suite caught me getting wrong on WebSocket before this landed.

## Breaking

Every `Interceptor` and `Pipe` implementor. An interceptor that only observes drops a semicolon; one that works after the call binds the answer and returns it. A pipe that falls through ends in `None`; one that rejects returns `Some`. A guard that set a custom rejection response reshapes with `#[catch(GuardRejection)]`, which already took precedence over it.

A handler can no longer set a response, so the precedence rule between a set response and a returned one is gone, along with the warning that announced which had won. The test pinning it is deleted rather than migrated.

## Deferred

`abort()` is left alone here. It is now a third spelling of what `bool` and `Some(R)` say more precisely, it is honoured on RPC, WebSocket and gRPC but ignored by HTTP's guard loop, and it shares a name with the concept `CancellationToken` actually implements. Removing it reopens all four enhancer signatures, which the next change does anyway.

353 integration tests, 86 core unit tests, fmt clean, no new clippy warnings.
