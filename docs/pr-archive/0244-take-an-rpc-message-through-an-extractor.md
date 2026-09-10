# #244 — Take an RPC message through an extractor

Merged 2026-09-05 into `master` from `refactor/rpc-takes-one-extraction-path`, commit [`0a9f519`](https://github.com/ulo-rs/ulo/commit/0a9f5199add0b61c41bf60c734385c3aca869dd4).

Every parameter of an RPC handler is now a `FromContext<RpcContext>`, and the message arrives through an extractor that says so:

```rust
#[message_pattern("orders.create")]
async fn create(&self, Payload(order): Payload<CreateOrder>) -> Result<OrderId, RpcError>
```

The bare form — a parameter of any unrecognised type being deserialised from the call's data — is removed, and the four-name list goes with it. That list existed only to pick between two places a parameter's value could come from:

```rust
if is_known_extractor(ty) {                 // RpcData | Extensions | Payload | Validated
    <Ty as FromContext<RpcContext>>::extract(ctx).await
} else {
    ctx.data().parse::<Ty>()
}
```

What it cost was diagnostics in both directions. A handler's own `struct Payload` was read as the framework's and told to implement a trait it had never heard of; an aliased `use Payload as P` took the other branch and failed on `DeserializeOwned` — a trait the author never mentioned. Neither message contained the name that caused it.

Both now produce the same thing, at the parameter:

```
error[E0277]: `Order` is not an extractor for `RpcContext`
  --> handlers.rs:17:35
   |
17 |     async fn handle(&self, order: Order) -> Result<u32, RpcError> {
   |                                   ^^^^^ cannot be extracted from this context
   = note: a handler parameter is read from the context it is handed. Take one of the
           framework's extractors — `Payload<T>` for a message, `Json<T>` or `Query<T>` on
           HTTP — or implement `FromContext<RpcContext>` for `Order`.
```

- **`#[diagnostic::on_unimplemented]` on `FromContext`** carries that note, so it holds for a custom extractor on any transport rather than only where a macro could have written it.
- **RPC and WebSocket now have one rule**, stated the same way: every parameter is an extractor, one name is read (`&RpcContext`), and it is backed by the type it is passed at.
- **A custom extractor and a framework one are the same thing to the macro.** Neither is on a list, so `Payload<T>` may be aliased or shadowed without changing what a signature means.
- **`Validated<Payload<T>>` is unchanged** — it was already on the extractor branch.
- **Migration** — four handlers across the tests and examples spelled their message bare; each takes one wrapper. `Payload<T>` deserialises the call's data into `T`, which is what the removed branch did, so there is no behaviour to re-pin.
- **Docs** — ADR-0041, which completes ADR-0023 on this transport and satisfies ADR-0040 by removing the read rather than backing it. ADR-0040 described this fork in its audit table and weighed the bare form against a misspelling alone; both are corrected there rather than left describing a transport that no longer works that way.
