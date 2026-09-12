# 0043 — A gRPC method's shape comes from the proto

Status: accepted. Supersedes the decision of
[ADR-0042](0042-a-grpc-handler-asks-the-type-what-the-wire-carries.md).

## Context

`#[grpc_methods]` writes the tonic trait impl around handlers spelled in ulo's shapes
([ADR-0038](0038-a-grpc-handler-is-written-in-ulos-shapes.md)). The trait tonic generated declares
each method with its request type — `tonic::Request<GreetRequest>`, or
`tonic::Request<tonic::Streaming<GreetRequest>>` where the caller streams — and the impl has to
repeat it. A macro runs before name resolution, so it cannot learn a type from anything it reads.

ADR-0042 had it learn the type from the handler: the first parameter was the request, and the
signature was written as a projection off it, `<Payload<GreetRequest> as GrpcRequest>::Arg`, for
the compiler to resolve. That made one parameter carry a rule the other three transports have
nowhere. `Payload<T>` was a `FromContext` on HTTP, RPC and WebSocket and a `GrpcRequest` on gRPC:
one spelling, two mechanisms, and a position rule to say which parameter the second one applied to.

The type is a fact of the proto, not of the handler. The proto is also the one place where a
`tonic::Streaming` request and a single message are told apart before any handler is written.

## Decision

**The build step writes what each method carries, and the macro reads that.** `ulo-build` runs
after tonic's own codegen and appends a companion module beside each service's `*_server` module,
one marker per method:

```rust
pub mod greeter_ulo {
    pub struct GreetAll;
    impl ::ulo_grpc::MethodShape for GreetAll {
        type Arg = ::tonic::Streaming<super::GreetRequest>;
        fn install(request: ::tonic::Request<Self::Arg>, ctx: &::ulo::grpc::GrpcContext) {
            ::ulo_grpc::shape::stream(request, ctx)
        }
    }
}
```

It reads the types off the trait tonic wrote, verbatim. No naming rule is re-derived: well-known
types, `extern_path` remaps and `super::` depth come out as tonic spelled them, and the companion
sits at the same depth so they resolve to the same items. The rewrite is in place, so the
`include_proto!` line and everything after it are unchanged; a `.proto` edit reruns the build
script and the companion with it.

`#[grpc_methods]` derives the module from the trait path — `pkg::greeter_server::Greeter` names
`pkg::greeter_ulo`, the trait snake-cased the way tonic names its own modules — and the marker
from the method name. The generated signature projects through the marker:

```rust
async fn greet_all(&self, request: ::tonic::Request<<pkg::greeter_ulo::GreetAll as MethodShape>::Arg>)
    -> Result<::tonic::Response<GreetReply>, ::tonic::Status>
```

**Every parameter of a handler is a `FromContext<GrpcContext>`, in any order.** The generated body
hands the request to the execution through the marker's `install`, which erases it into a slot on
the context, and then extracts each parameter as the other three transports do. `Payload<T>` takes
the message from the slot, `Inbound<T>` the caller's stream with tonic's statuses mapped to ulo's,
`ulo_grpc::GrpcRequest<T>` the whole request as tonic decoded it. The slot is taken once, so all
three declare `CONSUMES`, and the macro asserts per pair of parameters that at most one takes it —
the HTTP body's rule, applied to the one thing a gRPC call carries that cannot be handed out twice.

**What was a compile error stays one, except for one case.** A stream asked for on a unary method,
or a message on a streaming one, still fails against the proto: the slot holds what the marker
installed, and the wrong extractor reports `Internal` naming both types on the first call rather
than failing to compile. A `Payload<T>` whose `T` is not the method's message fails the same way.
The typed move ADR-0042 had is a downcast that cannot fail when the handler is right.

**The build step is required for `#[grpc_methods]`.** Generated code names the marker, so a crate
without the companion does not compile. A trait tonic did not write — hand-written, or generated
where the build script cannot reach — gets its companion from `ulo_build::shapes_in_file`.

## Consequences

- No parameter has a position, and no name is read off a handler but `&GrpcContext`. gRPC now
  states the rule ADR-0041 states for RPC, in the same words.
- `GrpcRequest` the trait is gone with the position rule; `GrpcRequest<T>` the newtype is the
  whole-request extractor, so a handler spelling `tonic::Request<T>` now spells that. ADR-0042's
  road-not-taken paragraph said the whole-request form could not survive extraction; it survives
  as this type, rebuilt from nothing because the carrier keeps the request whole.
- `build.rs` gains one line, `ulo_build::shapes("pkg")`, taking the string `include_proto!`
  takes. A crate that already had to depend on `ulo-grpc` to compile (ADR-0042) now also has a
  build-dependency on `ulo-build`.
- One `Box`, one lock and one downcast per call, beside tonic's own per-call allocations.
- `#[grpc_stream]` stays. It marks the reply, which changes what code the macro emits, and no type
  can tell a macro that at expansion. A manifest the build step writes and the macro reads could
  carry it; that is a follow-up, with the build step verified to re-expand on a proto change.
- Guards still do not see the message: the request is installed after the enhancer wrapper has
  run them. Installing before the guards, so a guard could take it under the same one-taker rule,
  is possible and not done.

## Roads not taken

**Composing generators inside tonic-prost-build.** Its `Builder::service_generator()` is public,
but the builder's fields are not, and `compile_with_config` re-applies them and installs its own
generator over any the caller set. Wrapping it means re-implementing its surface. Reading the file
it wrote costs nothing it does not already promise.

**A compile-time check that a parameter's `T` is the method's message.** Buildable on stable Rust
with a trait each gRPC extractor implements to say which message it wants, and coherent because a
"wants nothing" marker is local. It costs every custom gRPC extractor one more line, and it was
left for a follow-up rather than shipped with the mechanism it guards.

**A `#[request]` marker, or keeping the position with the build step beside it.** Either is a
second way of learning the type. ADR-0042's rule survives in the diagnostic only: a parameter that
is not an extractor fails at the parameter, as it did.
