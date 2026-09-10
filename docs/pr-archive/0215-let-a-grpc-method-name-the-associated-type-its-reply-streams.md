# #215 — Let a gRPC method name the associated type its reply streams

Merged 2026-08-31 into `master` from `feat/grpc-stream-optin`, commit [`d1fc9fe`](https://github.com/ulo-rs/ulo/commit/d1fc9fe8952f3a54ac2c37f84efd8515f5a0d4e6).

`#[grpc_methods]` recognises a streaming method from its response type naming `Self::SomeStream`, or from the pairing tonic-build creates between a method's name and its associated type. Neither reaches a trait whose own naming does not connect the two, and that trait is reachable through tonic's own second codegen path: `tonic_build::manual` takes the Rust name and the route name as independent builder fields, so `async fn watch` can answer on the route `StreamProgress` beside `type StreamProgressStream`. A hand-written trait can name them anything.

Such a method states it:

```rust
#[grpc_methods]
#[tonic::async_trait]
impl Watcher for Progress {
    type StreamProgressStream = Pin<Box<dyn Stream<Item = Result<Tick, Status>> + Send>>;

    #[stream(StreamProgressStream)]
    async fn watch(&self, r: Request<WatchRequest>)
        -> Result<Response<TickStream>, Status> { … }
}
```

The attribute is read before both inferred signals, so a service generated from a `.proto` declares nothing and an exceptional one declares it on the method it applies to. A name that is not an associated type of the impl block is refused, naming what it was checked against.

## Tests

`grpc_stream_optin.rs` serves a `tonic_build::manual` service whose route name and method name diverge — the shape the proto path cannot express. Two services differ only in the attribute: the one carrying it cancels the task feeding an abandoned reply, and the one without it does not, which is what makes the first assertion mean anything.

The fixture is generated in `build.rs` through `tonic_build::manual`, alongside the existing proto compilation.

ADR-0033's decision section records the rule the attribute completes.
