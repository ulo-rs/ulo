# #237 — Hand a gRPC handler the caller's stream as Inbound

Merged 2026-09-04 into `feat/grpc-streaming-handlers` from `feat/grpc-inbound-streams`, commit [`bf05fd5`](https://github.com/ulo-rs/ulo/commit/bf05fd570edbaaeb223b343cefbe92f0f10699a8).

A client-streaming or bidirectional handler reads the caller's messages as its own type:

```rust
#[grpc_method]
async fn greet_all(&self, mut inbound: Inbound<GreetRequest>) -> Result<GreetReply, NoName> {
    let mut names = Vec::new();
    while let Some(item) = inbound.next().await {
        names.push(item?.name);
    }
    Ok(GreetReply { message: names.join(", ") })
}
```

`Inbound<T>` is a stream of the message type whose items fail with a `GrpcStatus` rather than tonic's, so a handler reading one names nothing from the wire crate. The conversion sits where the macro unwraps the request, which is the only place that has to know tonic's shape.

- **Core** — `toni::extractors::Inbound<T>`, a boxed `Stream<Item = Result<T, GrpcStatus>>`.
- **Macro** — an `Inbound<T>` parameter makes the generated request `tonic::Request<tonic::Streaming<T>>` and maps each item's status on the way in. Combined with `#[grpc_stream]` it is a bidirectional method, so the shape is read entirely from the signature rather than from a third marker.
- **Tests** — client streaming answers once over three inbound names; bidirectional answers each as it arrives, which holds only if the reply stream is driven while the request stream is still open.

All four call shapes are now expressible in this form. The trait-impl form is untouched.

Extends ADR-0038.

Depends on #236.
