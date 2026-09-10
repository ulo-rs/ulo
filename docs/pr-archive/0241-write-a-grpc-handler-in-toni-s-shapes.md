# #241 — Write a gRPC handler in toni's shapes

Merged 2026-09-04 into `master` from `feat/grpc-stream-assoc-override`, commit [`b347de6`](https://github.com/ulo-rs/ulo/commit/b347de6091dedc3c9de54de4717c2a0ab775f052).

A gRPC handler now reads like a handler on the other three transports. `#[grpc_methods(proto::Trait)]` goes on the inherent impl holding the handlers and writes the tonic trait impl around them, so the signature is toni's rather than tonic's:

```rust
#[grpc_methods(greeter_server::Greeter)]
#[use_error_handlers(NoNameHandler)]
impl GreeterService {
    #[new]
    pub fn new() -> Self { Self {} }

    #[grpc_method]
    async fn greet(&self, Payload(req): Payload<GreetRequest>, ctx: &GrpcContext)
        -> Result<GreetReply, NoName>
    {
        if req.name.is_empty() { return Err(NoName); }
        Ok(GreetReply { message: format!("{} on {}", req.name, ctx.method()) })
    }
}
```

- **What a handler takes** — `Payload<T>` or the message written bare, `Inbound<T>` for a request the caller streams, `Extensions` for the execution's bag, `&GrpcContext`, and `tonic::Request<T>` where it wants trailers or the peer address. A parameter naming none of those is read as the request message, the way an RPC handler spells its payload.
- **What a handler answers** — the reply message, `tonic::Response<T>` where it sets reply metadata itself, or no `Result` at all where it cannot fail. Its error implements `toni::Error` and is parked on the execution on the way out, so `#[catch(MyError)]` matches here as it does everywhere else.
- **Streaming** — `#[grpc_stream]` marks a streaming reply; the handler yields its own item type and the macro declares the associated type the trait asks for, redeclared as `ScopedGrpcStream` so an abandoned reply still fires the execution's token. A trait whose method and stream names do not pair names it on the attribute: `#[grpc_stream(StreamProgressStream)]`. All four call shapes are read from the signature — bidirectional is the two streaming answers together, not a third marker.
- **The trait-impl form is removed.** Annotating `impl Trait for Service` is an error naming the form to write instead. `GrpcFail::fail` and `FailWith::fail_with` go with it: both answer with a `tonic::Status`, which no handler can return. `to_status` stays, for a service registered through `GrpcAdapter::add_service` and answering outside toni's dispatch.
- **Docs** — ADR-0038.

`Validated<Payload<T>>` is not among the parameters: proto messages are generated, so there is nowhere to hang the `#[validate]` attributes it reads.
