# #235 — Let a gRPC handler answer with its own reply type

Merged 2026-09-04 into `master` from `feat/grpc-handlers-read-like-handlers`, commit [`df0b6a2`](https://github.com/ulo-rs/ulo/commit/df0b6a21271e1fe00bafb7f22b4cb102c9fe445f).

A gRPC service can be written the way a service on the other three transports is written. `#[grpc_methods(proto::Trait)]` on an inherent impl generates the proto trait impl, so the handler stops spelling out the transport:

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

No `tonic::Request`, no `Response`, no `Status`, no `#[tonic::async_trait]`, and the constructor sits in the block with the handler — the shape `#[routes]` and `#[patterns]` already have.

- **Macro** — an inherent impl splits into the handlers under a hidden name and the trait impl that calls them, unwrapping the request, wrapping the reply, and mapping the error through `grpc_code`. The generated impl is then fed to the machinery that already existed, so guards, interceptors, declared metadata and panic recovery apply unchanged.
- **Errors** — the returned error is parked on the execution on its way out, so `#[catch(MyError)]` matches on this transport. That is ADR-0037's mechanism, now reached without the handler calling anything.
- **Tests** — a one-rpc `Greeter` service exercised end to end: the reply reads back `ctx.method()`, and a domain error is claimed by a handler that downcasts it.

The trait-impl form is untouched and still compiles, so a service that streams keeps writing what it writes today. Streaming in the new form is deferred: a trait impl must satisfy every method its trait declares, so the macro has to write those before a streaming proto can use it.

Naming the trait is required rather than inferred. An inherent impl has no header to read it from, and guessing a module path from a struct's identifier fails as an error naming a path the author never wrote.

Implements ADR-0038.
