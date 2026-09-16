# 0048 — A gRPC reply travels through the pipeline

Status: accepted

Revisits the consequence [ADR-0033](0033-a-grpc-streaming-reply-is-part-of-its-execution.md) drew
from a gRPC reply's type not being the framework's.

## Context

An enhancer on this transport was typed `Interceptor<GrpcContext, Result<(), GrpcStatus>>`, and the
reply left the handler by a different road: the generated wrapper stashed the user's
`Result<Response<T>, Status>` in an `Arc<Mutex<Option<_>>>` and read it back once the pipeline had
returned. The reason is recorded in ADR-0033 — a gRPC reply is typed by the tonic trait's own
associated types, so it is the method's type, not the framework's, and one interceptor list serves
every method of a service.

Three things followed from the reply not being in the answer.

**An interceptor could not see it, replace it, or produce one.** A cache hit answering without
running the handler, a response envelope, logging what went back — each is written the same way on
HTTP, RPC and WebSocket and was unwritable here.

**An interceptor could not see a failure either.** A handler's `Err` went into the same
side-channel, so `next.run(ctx)` answered `Ok(())` whether the handler had replied or failed.

**A claim and a decline were the same value.** An error handler is instantiated at its transport's
answer type, so a claim here answered `Result<(), GrpcStatus>`: an `Ok` carrying `()`, which no
method replies with. The runtime treated it as it treated `None`, and walked on to the next
handler.

Separately, the chain ran at three sites, below the interceptors, inside the handler step. An
interceptor's deliberate `Err` therefore reached the wire with no `#[catch]` handler offered it,
while the same interceptor panicking was routed through the chain. HTTP, RPC and WebSocket each
closed that by running the chain once above the interceptors. gRPC could not be swept in with them:
the type a claim answers carries no reply, and hoisting alone leaves `Ok` naming nothing.

## Decision

**Every way a call can fail leaves as `Err(GrpcStatus)` carrying its cause, and the chain runs once
above the interceptors.** A refusal carries its `GuardRejection`, a panic anywhere below carries its
`PanicRecovered`, a handler's failure carries the domain error it raised — `GrpcStatus` has held a
`source` since [ADR-0039](0039-a-grpc-status-carries-the-error-it-came-from.md), and that slot is
what carries the type the chain is offered. One seam reads it:

```rust
let observed = status.source().unwrap_or(&status);
match claim::<Grpc>(&handlers, observed, ctx).await {
    Some(answer) => answer,
    None => Err(status),
}
```

**The reply travels in the pipeline's `Ok`, erased.** `GrpcReply` holds the value as
`Box<dyn Any + Send>` beside the name of the type it carries, and `GrpcHandlerResult` becomes
`Result<GrpcReply, GrpcStatus>`. It is built where `tonic::Response<T>` is nameable — in the user's
crate, by the generated wrapper — and reaches core erased, the way a request already does through
`RequestCarrier` (ADR-0043). Core still names no tonic type.

**`Ok` means a reply wherever it appears.** An interceptor downcasts it, replaces it, or builds one
without calling `next`; an error handler claiming a failure with `Ok(reply)` recovers the call. The
decline is `None`, and it is the only spelling.

## Consequences

The two side-channels in the generated wrapper are gone, and with them the reroute that carried a
caught panic out to be offered separately. `GrpcStatus` flattens into `tonic::Status` at one place —
the trait method a caller reaches without the wrapper — rather than being flattened on the way out
of every handler and re-inflated by the wrapper to recover the error.

An enhancer names the reply's type itself, so answering with the wrong one fails that call with
`Internal` naming both types rather than failing to compile. That is the request side's trade
(ADR-0043) on the reply side, and it is what one interceptor list per service costs.

Breaking for a hand-written `impl Interceptor<GrpcContext, GrpcHandlerResult>` or
`ErrorHandler<GrpcContext, GrpcHandlerResult>`, and breaking at compile time in both directions: an
interceptor that passed `next.run(ctx)` through is unchanged, and one that wrote `Ok(())` — a chain
handler declining that way included — stops compiling, because `()` is not a `GrpcReply`. An
enhancer that spelled the answer type out rather than naming the alias compiles and is no longer
detected as an enhancer, which startup reports by name. Nothing changes for a handler, a guard, or a
`#[catch]` function.

## Roads not taken

**Monomorphise the interceptor list per method.** Exact, and it types the reply. It gives up one
list covering a service, which is how enhancers are declared and resolved on every transport.

**Name the reply on the ulo-build marker.** A `type Reply` beside `type Arg` is writable and would
name the type for a diagnostic. It does not make a list that serves every method of a service
typed, which is the live blocker.
