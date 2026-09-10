# #151 — A context is a shared handle, and enhancers take it by reference

Merged 2026-08-17 into `master` from `feat/contexts-are-shared-handles`, commit [`2b203d9`](https://github.com/ulo-rs/ulo/commit/2b203d9860537851744db8c543eff7074c90e431).

Enhancers took `&mut C` because `&T` is `Send` only where `T` is `Sync`, and a context was not `Sync`. The exclusive reference was never about exclusivity — it was the shape a bound forced.

The bound came from two fields. `HttpContext`'s request and response bodies are `UnsyncBoxBody`; every other field on every context was already `Sync`. A `Mutex` over a `Send`-only body *is* `Sync`, so the question goes away without reimposing anything on user streams.

Each context is now an `Arc`-backed handle — `Send + Sync + Clone + 'static`, cheap to clone, shareable with whatever outlives the handler. `HandlerContext` regains the `Sync` supertrait it lost for the opposite reason.

```rust
#[derive(Clone)]
pub struct HttpContext { inner: Arc<HttpInner> }

struct HttpInner {
    shared: SharedState,
    parts:  RequestPart,
    body:   Mutex<Option<RequestBody>>,   // the only lock on any context
}
```

## Components

- **Traits** — `Guard`, `Interceptor`, `InterceptorNext`, `Pipe`, `ErrorHandler`, `ErrorObserver`, `FromContext`, and `Route::execute` / `GatewayTrait::handle_event` / `RpcControllerTrait::handle_message` all take `&C`.
- **`#[catch]`** inverts: it demanded `&mut CtxType` and rejected a shared reference; now the reverse, and its two argument checks collapse into one.
- **Macros** — the three context-passthrough emissions and the four generated handler signatures.
- **Tests** — one new assertion that every context is `Send + Sync + Clone + 'static`. A context losing `Sync` breaks the entire enhancer surface, and the error would land far from the cause.

## Deletions

Three pieces of per-execution state are removed rather than wrapped in locks they don't need. The move to handles is what exposed them — every field had to end up immutable, locked, or gone.

- **`SharedState.dto`** — written on every validated request, read by nothing. `set_dto` had one caller and the getter had none. Validation is untouched: the 400 is the point, and `Validated<T>` is what reaches a handler.
- **`RpcContext.transport_extensions`** — allocated per call, never written, never read. Its doc described carrying the original NATS message handle. The bag on `HandlerContext` is the same `TypeId`-keyed shape and is where one belongs.
- **`metadata_mut`** — one caller across RPC and gRPC, immediately after construction, so the value goes to the constructor instead.

**`abort()` goes with them.** Nest's `ExecutionContext` has no such thing, so it was toni's own. Once enhancers began answering by returning, it said nothing `bool` and `Some(R)` don't say more precisely — and it was honoured on RPC, WebSocket and gRPC while HTTP's guard loop never checked it, so a guard aborting worked on three transports and was ignored on the fourth. It also shares a name with the concept `CancellationToken` implements.

## Two doc comments corrected

Both asserted behaviour that does not exist. `CancellationToken` claimed to be "signalled when the request ends or the client disconnects" — nothing in the framework signals it; firing on disconnect needs a producer in each adapter. `deadline()` claimed "gRPC populates this from `grpc-timeout`" — no transport overrides it, so it is `None` everywhere.

## Breaking

Every `Guard`, `Interceptor`, `Pipe`, `ErrorHandler` and `ErrorObserver` implementor: `&mut C` becomes `&C`. Every custom extractor: `FromContext::extract` likewise. `#[catch]` functions take `&CtxType`. Anything calling `abort()` / `should_abort()`, `set_dto()` / `dto()`, `transport_extensions()`, or `metadata_mut()` has no replacement — a guard rejects by returning `false`, a pipe answers with `Some`, and per-execution values go in the extension bag.

`RpcContext::new` takes the call's metadata as an argument.

353 integration tests, 87 core unit tests, fmt clean, no new clippy warnings.
