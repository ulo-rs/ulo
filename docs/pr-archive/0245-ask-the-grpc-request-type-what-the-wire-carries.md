# #245 — Ask the gRPC request type what the wire carries

Merged 2026-09-06 into `master` from `feat/grpc-params-are-extractors`, commit [`9461815`](https://github.com/ulo-rs/ulo/commit/946181589b61cd68a868565c93184452fdd2589a).

`#[grpc_methods]` writes the proto trait's signature, which names the message type. It read that type off the parameter's last path segment, and a proc macro runs before name resolution — so `use toni::extractors::Payload as P` arrived as the identifier `P`, fell to the bare-message case, and generated `tonic::Request<Payload<GreetRequest>>`.

The request is now the first parameter, and the signature names its message through a projection:

```rust
async fn greet(&self, request: ::tonic::Request<<Payload<GreetRequest> as GrpcRequest>::Arg>)
    -> Result<::tonic::Response<GreetReply>, ::tonic::Status>
```

rustc normalizes that against the trait's own declaration. `Payload<T>` projects to `T`, `Inbound<T>` to `tonic::Streaming<T>`, `tonic::Request<T>` to `T` — aliases and re-exports included, because the compiler does the resolving.

Everything after the request is a `FromContext<GrpcContext>`, which is what HTTP, RPC and WebSocket already do. `&GrpcContext` passes through, the one name still read on any transport, backed by being passed at that type.

- **`classify_param`, `claim_request`, `HandlerParam` and `RequestKind` are gone** with the reading they existed for. Which call shape a method serves comes from the request parameter's projection and the presence of `#[grpc_stream]`, neither of which is a name.
- **Breaking: the bare-message form is removed.** It cannot survive the trait — `impl<T: Message> GrpcRequest for T` overlaps the impl for `Payload<T>`, since coherence must assume an upstream crate could implement `Message` for it, and specialization is unstable. A handler takes `Payload<T>`, the spelling every other transport uses.
- **Breaking: the request comes first.** Every handler in the tree already wrote it there.
- **`GrpcRequest` lives in `toni-grpc`.** Its impls name `tonic::Request`, which toni core does not depend on, and a trait declared in core could not be implemented in `toni-grpc` for `Payload<T>` — both foreign there. Generated code therefore names `::toni_grpc::GrpcRequest`, so a crate writing `#[grpc_methods]` depends on `toni-grpc` to compile rather than only to serve.
- **Two parameters cannot both take the request:** only the first is one, and a second `Inbound<T>` is extracted through `FromContext<GrpcContext>`, which it does not implement.
- **Docs** — ADR-0042, with ADR-0038's parameter paragraph annotated where this supersedes it.

`a_handler_names_its_request_through_an_alias` serves a call whose request is spelled `Aliased<GreetRequest>`; removing the projection fails every gRPC handler in the tree, that one included. `GreetBare` becomes `GreetWithBag` — the form it was named for is gone, and what it still pins is that a guard's write reaches the handler's `Extensions`.
