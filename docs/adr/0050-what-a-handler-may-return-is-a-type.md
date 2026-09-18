# 0050 — What a handler may return is a type

Status: accepted

Moves the decision of what a handler may answer with out of three mechanisms and into one trait,
and closes the seam where a return type's spelling decided whether an error reached the chain.

## Context

Each transport decided what a handler may return in its own way, and the four had nothing in
common but the question:

| Transport | how it decided |
| --- | --- |
| HTTP | `IntoResponse`, a trait with an impl per returnable type |
| RPC | `#[patterns]` reading the written return type, three branches |
| WebSocket | one bare type, no conversion |
| gRPC | the proto, through `#[grpc_methods]` reading the signature |

Three of them read a **spelling**, and on HTTP the reading decided where an error went.
`#[patterns]` matched the last path segment of the return type against `RpcHandlerOutput` and
`RpcData` and serialized anything else. `#[routes]` asked whether the written return type was
`Result<_, _>` and split it when it was. `#[grpc_methods]` reads one too, to tell a fallible handler
from an infallible one, and nothing about where an error goes hangs on it.

Reading the spelling is what ADRs [0040](0040-a-name-the-framework-reads-is-backed-by-a-type.md) and
[0041](0041-an-rpc-handler-reads-its-parameters-through-extractors.md) removed from the parameter
side. On the return side it was also wrong, and not only untidy. A handler written
`-> Result<T, E>` had its error lifted into `HttpError` and offered to the error chain. The same
handler written `-> impl IntoResponse`, returning the same `Err`, took the other branch, and the
blanket `impl<T, E> IntoResponse for Result<T, E>` rendered the error itself: a `#[catch]` handler
registered for that error was never offered it, and nothing at either call site named the
difference. `impl IntoResponse` was the spelling the `#[controller]` documentation showed first.

The match was on the last path segment, so a return type written as an alias for a `Result` —
`type ApiResult = Result<Body, MyError>` — read as infallible and took the same branch.

## Decision

**What a handler may return is decided by its type.** `IntoOutput<T>` is the trait that decides it,
with many impls per transport, and `Transport::Output` is what the pipeline carries, one per
transport ([ADR-0049](0049-an-answer-is-an-envelope-and-a-cardinality.md)).

```rust
pub trait IntoOutput<T: Transport> {
    fn into_output(self) -> Answer<T>;
}
```

Conversion answers `Answer<T>` rather than `T::Output` because one transport's conversion runs
serde and can fail. A failure is that transport's own error and reaches the chain like any other.

**A handler's `Result` is one impl, and it is what keeps `#[catch]` reachable.**

```rust
impl<T: Transport, V: IntoOutput<T>, E: Into<T::Error>> IntoOutput<T> for Result<V, E>
```

An error a handler returns becomes `T::Error`, and the only path from a handler's `Err` leads to
the error side. No macro matches a return type against a name to decide how to convert a value, so
no spelling routes around the chain. No conversion can render an error: none takes one. `HttpError`
implements `IntoOutput<Http>` nowhere, and a handler cannot answer with one as its value.

**`IntoResponse` is retired, and HTTP names one trait like the others.** Its impls are
`IntoOutput<Http>` impls — `HttpResponse`, `Body`, `u16`, `Vec<(String, String)>`, `(u16, Body)`,
`serde_json::Value`, `String`, `&'static str`, `Sse`, and `HealthCheckResult` in `ulo-health`. A
second trait bridged by a blanket is what HTTP had and RPC and WebSocket did not, and the name it
added said less than `IntoOutput<Http>`, which names the transport.

**RPC's serialize fallback is chosen by method resolution, not by a bound.** A blanket
`impl<S: Serialize> IntoOutput<Rpc> for S` cannot coexist with an impl for `RpcData` or for
`Result`, because both are themselves `Serialize` — `E0119` on each. Specialization is unstable, so
the two candidates are separated by autoref depth instead:

```rust
impl<T: IntoOutput<Rpc>> Answered  for &&Answers<T>      // tried first
impl<T: Serialize>       Serialized for &Answers<T>      // reached only if the first does not apply
```

The macro emits `(&&Answers::new(value)).ulo_answer()`. A type that says what it is wins; anything
else is serialized. The pair is macro ABI rather than vocabulary: it lives in `ulo::__rpc::answer`
beside the other bridges `#[patterns]` emits against, and a handler names none of it. This keeps a
handler's freedom to return a plain DTO while `RpcData` still means `RpcData` — serializing an
`RpcData` would wrap it in its own enum tag, since it is externally tagged, and produce
`{"Text": "hi"}` where the handler said `Text("hi")`.

**gRPC keeps the proto.** Its answer type is the method's, tonic's trait names it, and a handler
that returned something else would be answering a different method. That is the fact every gRPC
divergence in this framework descends from, and it is not a mechanism to be unified away.

## Consequences

One mechanism decides what three transports accept, and it is the type system rather than a macro
reading tokens. Adding a returnable type is an impl on any of the three, where on RPC it was a
branch inside `#[patterns]`.

Neither macro matches a return type against a name to decide how to convert a value.
`returns_result_type`, `returns_rpc_handler_output` and `returns_rpc_data` are gone, and with them
the class of defect where two spellings of one type mean two things. Two return-type reads remain
and neither decides where an error goes: `#[routes]` tells an infallible SSE stream from a fallible
one, and `#[patterns]` checks that an `#[event_pattern]` handler returns `Result<(), _>`.

A WebSocket handler may answer `WsMessage` or `()` directly, where it named `WsHandlerOutput` for
both.

**Breaking, in three shapes, and the compiler names two of them.** `IntoResponse` is gone: a
handler written `-> impl IntoResponse` is written `-> impl IntoOutput<Http>`, and a type made
returnable by implementing `IntoResponse` implements `IntoOutput<Http>` instead, answering
`Answer<Http>` rather than `HttpResponse`. A handler that answered with an `HttpError` as its value
no longer compiles, because nothing makes an `HttpError` an output. A handler whose return type is
an alias for a `Result` compiles unchanged and now reaches the error chain where it rendered its
own error, with no diagnostic — the old check matched the last path segment against `Result`, and
an alias is not one.

`IntoOutput` and `Http` are `ulo::dispatch` items, re-exported by neither `ulo::http` nor the
prelude, so a handler that imported `ulo::http::IntoResponse` imports `ulo::dispatch::{Http,
IntoOutput}` instead.

## Roads not taken

**Drop RPC's serialize fallback.** It removes the coherence problem by removing the feature: every
handler would name `RpcData` or build the count itself. The fallback is what lets an RPC handler
return a domain type the way an HTTP one returns `Json`, and it is documented as the idiom.

**Keep `IntoResponse for Result` and have the macro route around it.** The macro would go on
reading the return type, which is the thing being removed, and the blanket would stay reachable by
anyone calling `into_response` directly.

**Make an error type returnable by routing it to `Err`.** `impl IntoOutput<Http> for HttpError`
answering `Err(self)` is the correct semantics for an error in the value position: the call failed,
and the chain sees it. The objection is to what it adds, not to what it does.

It is a second spelling for what `Err(e)` already says, which is the duplication this decision
removes elsewhere. It also makes `Ok(HttpError::not_found("x"))` well-formed through the `Result`
impl, mapping to `Err`, so a handler writes `Ok` and the call fails with nothing naming the
inversion. Refusing answers the same case with a compile error and a one-token fix, and a loud
refusal in a rare case beats a silent surprise in one.

It would also blur a distinction worth keeping. An `HttpResponse` carrying 404 is a handler
succeeding at saying "not found"; an `HttpError::not_found` is a handler failing. Both reach the wire
as 404, and only the second passes the chain. Which of the two a value means should not depend on
where it sits.

The shape is not an idiom on any transport. `RpcError` reaches neither autoref arm, implementing
neither `IntoOutput<Rpc>` nor `Serialize`, and `WsError` has no impl either. Revisit only if a
handler that always fails is wanted as a declared shape — then it is one rule across all three
transports, and what `Ok(an error)` means is settled before it ships.

**Keep `IntoResponse` as an alias for `IntoOutput<Http>`.** Trait aliases are unstable, so the
spelling would be a supertrait with a blanket impl, and it would work in bound position only: a type
is still made returnable by implementing `IntoOutput<Http>`. It would also keep the signature that
put an error in the wrong place. An infallible `fn into_response(self) -> HttpResponse` has nowhere
for a failure to go, which is why the impl it had for `Result` rendered the error rather than
propagating it, and `Answer<Http>` leaves one place for one to go. The name would carry nothing the
bound does not, and RPC and WebSocket would each be owed one for no mechanism.

**Give gRPC an `IntoOutput<Grpc>`.** Nothing would implement it but the method's own message type,
and the impl could not be written generically, because which message is correct depends on which
method is being answered.
