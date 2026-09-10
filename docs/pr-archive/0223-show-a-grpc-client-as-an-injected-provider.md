# #223 — Show a gRPC client as an injected provider

Merged 2026-08-31 into `master` from `docs/grpc-client`, commit [`3a31811`](https://github.com/ulo-rs/ulo/commit/3a3181136202d5948bf2d30faa3e5b5ea67922df).

Nest hands out a typed gRPC client through `ClientsModule.register(...)`, and toni has no equivalent module — for gRPC or for any transport, since an `RpcClient` is constructed in a provider too. That reads as a missing capability, and it is not one.

```rust
#[module(
    controllers: [OrdersGateway],
    providers: [provider_factory!(OrdersClient<Channel>, || {
        OrdersClient::new(Channel::from_static("http://orders:50051").connect_lazy())
    })]
)]
impl AppModule {}

#[controller("/orders")]
pub struct OrdersGateway {
    #[inject]
    orders: OrdersClient<Channel>,
}
```

Registered under its own type, so the injection needs no string token. Nothing framework-side is involved, and nothing needed adding to make this work.

`connect_lazy` is the same contract the RPC client transports document — no I/O in a constructor. Startup does not depend on the peer being up, and reaching a dead peer is an ordinary handler error rather than a failed boot, which the example shows by rendering the status rather than unwrapping it.

## Why no module

ADR-0034 records the rule this follows: an integration earns a module when something has to own a
lifetime the container cannot infer from a constructor expression — a pool, a startup check, a
shutdown hook, a health indicator, a reserved token. A client owns none of those, and the two
decisions that put it on that side (lazy connect, no ambient per-call state) are decisions this
framework already made.

## What this adds

- `examples/grpc_client.rs` — an HTTP route calling a gRPC service through an injected client. It serves both halves so it runs with one command; a real deployment changes the URL and nothing else.
- `grpc_client_injection.rs` — pins the wiring end to end against a running toni gRPC server.
