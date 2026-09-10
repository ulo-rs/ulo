# #240 — Serve gRPC only from the handler form

Merged 2026-09-04 into `feat/grpc-stream-assoc-override` from `refactor/grpc-one-handler-form`, commit [`b18efe0`](https://github.com/ulo-rs/ulo/commit/b18efe06bd4b56e273421fb9ac170920619418e4).

`#[grpc_methods]` on a trait impl is removed. A gRPC service's handlers live in an inherent impl: they take `Payload<T>` or `Inbound<T>` and answer with the reply message, and the macro writes the tonic trait impl around them. Annotating a trait impl is an error naming the form to write instead.

Two ways to serve one rpc mean two sets of diagnostics and two answers to every question about what a handler may take. A service written against tonic's signatures moves by naming the proto trait in the attribute, dropping `#[tonic::async_trait]` and the `impl Trait for` header, marking each method, and replacing `Request<T>`/`Response<T>` with the message types — or keeps its hand-written impl and registers through `GrpcAdapter::add_service`, outside toni's dispatch and its enhancers.

- **Macro** — the generated impl writes each streaming response as `Self::SomeStream`, which retires the two other signals a hand-written impl needed: the pairing between a method name and an associated type, and the attribute that overrode it where a `tonic_build::manual` trait broke that pairing.
- **`GrpcFail::fail` and `FailWith::fail_with`** — removed. Both answer with a `tonic::Status`, and a handler's error type implements `toni::Error`, which `tonic::Status` does not. Parking the domain error for the chain survives them: the generated method does it on every failed call, so `#[catch(MyError)]` still matches. `to_status` stays, for a service answering outside toni's dispatch.
- **Fixtures** — every gRPC fixture and example moves to the handler form. `grpc_stream_optin.rs` merges into `grpc_manual_trait_form.rs`; both pinned a trait whose method and stream names do not pair, one through each form.
