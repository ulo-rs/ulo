# #247 — Read a gRPC method's shape off the proto

Merged 2026-09-09 into `master` from `feat/grpc-shapes-come-from-the-proto`, commit [`d4377e6`](https://github.com/ulo-rs/ulo/commit/d4377e6aa0b4262dfaee1b45e9137ae0163f5034).

`#[grpc_methods]` learns what each proto method carries from the build step rather than from the handler. `toni-build` runs after tonic's codegen, reads the request types off the trait tonic wrote, and appends a companion module beside it, one `MethodShape` marker per method. The generated tonic impl projects through that marker for its signature and hands the request to the execution through it, so every parameter of a handler is a `FromContext<GrpcContext>`, in any order.

```rust
// build.rs
tonic_prost_build::compile_protos("proto/orders.proto")?;
toni_build::shapes("toni_examples.orders")?;
```

```rust
#[grpc_method]
async fn greet(&self, extensions: Extensions, Payload(req): Payload<GreetRequest>, ctx: &GrpcContext)
    -> Result<GreetReply, NoName>
```

- **`toni-build`**, a build-dependency. `shapes(package)` takes the string `include_proto!` takes and rewrites that file in place; `shapes_in_file(path)` covers a trait tonic did not write. Types are copied verbatim from the trait: well-known types, `extern_path` remaps and `super::` depth come out as tonic spelled them, and the companion sits at the same depth. A second run replaces its own section. `tonic_build::manual` output has the same trait shape and is covered.
- **Core.** `GrpcContext` holds the request erased in a one-shot slot behind `RequestCarrier`. `Payload<T>` and `Inbound<T>` gain `FromContext<GrpcContext>` impls that take it and downcast, each declaring `CONSUMES`. A wrong type answers `Internal` naming what the handler asked for and what the call carries; two takers in one handler fail to compile naming both, which is the HTTP body's rule applied to the request.
- **toni-grpc.** `MethodShape`, with `shape::message` and `shape::stream` for the companion to call. `GrpcRequest<T>` is the whole-request extractor. The `GrpcRequest` trait is gone with the position rule.
- **Macro.** No parameter has a position and no name is read but `&GrpcContext`. The companion module is derived from the trait path, `pkg::greeter_server::Greeter` to `pkg::greeter_toni`, and overridable with `shapes = path`. A method reached outside toni's dispatch answers `Internal` rather than panicking. `#[grpc_stream]` stays: it marks the reply, which changes what code the macro emits, and no type can tell a macro that.
- **Breaking.** `build.rs` needs the `toni_build::shapes` line and the crate a build-dependency on `toni-build`; `tonic::Request<T>` as a parameter becomes `toni_grpc::GrpcRequest<T>`.
- **Docs.** ADR-0043 supersedes ADR-0042's decision and corrects its road-not-taken paragraph on the whole-request form. The ADR index gains 0039 through 0043, which it had not listed.

`greet_with_bag` takes `Extensions` before the request, and `a_handler_names_its_request_through_an_alias` serves a call spelled `Aliased<GreetRequest>`. Every `grpc_handler_form` test fails with the install line removed, eight of ten with the "no request was installed" diagnostic. toni-build's tests cover the four call shapes and a repeated run, and share the naming table the macro's tests assert against.
