# #214 — Hold a gRPC execution across its streaming reply

Merged 2026-08-30 into `master` from `feat/grpc-streaming-tail`, commit [`0632e9d`](https://github.com/ulo-rs/ulo/commit/0632e9df5aa8c468471e2b9618437e89daa5b458).

A gRPC server-streaming or bidi reply outlives the handler that returned it. The execution ended when the method returned: the context died there, and a reply the caller abandoned left whatever fed it running. HTTP, WebSocket and RPC cover that tail under ADR-0021 and ADR-0032; gRPC was outside both.

## The reply carries the execution

The generated wrapper declares its own associated stream types — `ScopedGrpcStream<UserStream>` rather than an alias of the user's — so the context rides the reply to its last item, and a drop before then fires the execution's cancellation token. `ScopedGrpcStream` is generic over the inner stream and names no tonic type, leaving `grpc_runtime` tonic-free.

Carrying a reply to the wrapper's type needs no per-method code. Two impls resolve it by target type and cannot overlap:

```rust
impl<T> IntoScoped<T> for T                            // a message is already the right type
impl<S: Stream> IntoScoped<ScopedGrpcStream<S>> for S  // a stream is wrapped on the way out
```

Unary methods resolve to the identity impl at no cost.

## The context rides the request

A gRPC handler's signature is the tonic trait's and carries no context, so guards and interceptors receive one and a handler does not — the generated method put only the extension bag on the request. The token lives on the context, and firing it while it is unreachable signals where nothing can hear.

`#[grpc_methods]` now inserts the `GrpcContext` alongside the bag, and `GrpcContext::of(request.extensions())` reads it back:

```rust
let ctx = GrpcContext::of(request.extensions()).expect("dispatched by toni");
tokio::select! {
    _ = ctx.cancellation().cancelled() => return,
    _ = feed_the_reply() => {}
}
```

A gRPC handler also gains the declared metadata of ADR-0020, which it could not read before.

## Which methods are covered

A signature has two legal spellings for one stream type, and a macro reading tokens cannot tell they are the same. Both are read. `Self::SomeStream` in the response type is the direct evidence, and it is what tonic-build declares — every one of this repository's 40 streaming signatures. Where a signature names the concrete type, the method pairs with its associated type by name: tonic-build derives `watch_progress` and `WatchProgressStream` from one proto identifier and emits the associated type only for methods that stream. The wrapper's signature then restates that payload, being generated text under no obligation to copy the user's.

Neither signal reaches a hand-written trait whose associated type is named off that convention and whose method also avoids `Self::` — `type Feed` beside `async fn watch(…) -> Response<Feed>`. That reply passes through unwrapped, as every gRPC reply does today. Reading tonic-build's naming is what the macro already does to find the server type from the trait name.

`CancellationToken`'s rustdoc claimed nothing fires it and that a producer would have to live in each adapter. ADR-0021 made both false; corrected here.

Handler code is unchanged. `ScopedGrpcStream` never appears in user code, and nothing about what a service may return moves.

Design and the rejected alternatives are in ADR-0033.
