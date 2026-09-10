# #236 — Let a streaming gRPC handler yield its own item type

Merged 2026-09-04 into `feat/grpc-handlers-read-like-handlers` from `feat/grpc-streaming-handlers`, commit [`992156e`](https://github.com/ulo-rs/ulo/commit/992156efb4d62313f6f79160b56ca4e05bfbfeb5).

A server-streaming handler answers with a stream of its own types, and the macro writes the rest:

```rust
#[grpc_stream]
async fn greet_many(&self, Payload(req): Payload<GreetRequest>)
    -> Result<impl Stream<Item = Result<GreetReply, NoName>> + Send + 'static, NoName>
{
    if req.name.is_empty() { return Err(NoName); }
    Ok(stream::iter(…))
}
```

No `type GreetManyStream = Pin<Box<dyn Stream<…> + Send>>` to declare, no `Status` in the item type, no `Response` to wrap.

- **Macro** — `#[grpc_stream]` reads the item type off the `Item =` binding, declares the associated type the trait pairs with the method (`greet_many` ↔ `GreetManyStream`, the pairing tonic-build makes from one proto identifier), and boxes the handler's stream into it.
- **Errors** — the one that stops the stream opening takes the unary path and reaches the chain with its type. An item's error arrives after the answer has begun, so it maps to the code its kind means and goes on the wire — the split ADR-0032 records for an RPC reply stream.
- **The tail** — the associated type belongs to the macro, which is what lets the wrapper redeclare it as `ScopedGrpcStream`. A reply the caller abandons fires the execution's token here as it does for a hand-written impl.
- **Tests** — the items read back, a stream that fails to open is claimed by a chain handler, and an abandoned stream cancels the work feeding it. The last one fails when the wrapper stops redeclaring the associated type.

Client streaming and bidi are deferred: an inbound stream has no toni-shaped spelling yet, and giving it one is a type in core rather than a change to this lowering.

Extends ADR-0038.

Depends on #235.
