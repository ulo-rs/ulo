# 0050 — What a handler may return is a type

Status: accepted

Moves the decision of what a handler may answer with out of three mechanisms and into one trait,
and closes the seam where a return type's spelling decided whether an error reached the chain.

## Context

Each transport decided what a handler may return in its own way, and the four had nothing in
common but the question:

| Transport | how it decided |
| --- | --- |
| HTTP | `IntoResponse`, a trait with nine impls |
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
difference. `impl IntoResponse` is the spelling the `#[controller]` documentation shows first.

The match was on the last path segment, so a return type written as an alias for a `Result` —
`type ApiResult = Result<Body, MyError>` — read as infallible and took the same branch.

## Decision

**What a handler may return is decided by a trait, and what the pipeline carries is an associated
type.** `Transport::Output` is one per transport ([ADR-0049](0049-an-answer-is-an-envelope-and-a-cardinality.md));
`IntoOutput<T>` has many impls per transport.

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
no spelling routes around the chain. One conversion still renders an error — `IntoResponse for
HttpError` — and a handler reaches it only by putting an `HttpError` in the value position, which
says to render it.

`IntoResponse` stays as HTTP's own vocabulary and gains `impl<T: IntoResponse> IntoOutput<Http> for T`.
Its impls are unchanged and a user's own keeps working. What it loses is `IntoResponse for Result`,
which is the defect.

**RPC's serialize fallback is chosen by method resolution, not by a bound.** A blanket
`impl<S: Serialize> IntoOutput<Rpc> for S` cannot coexist with an impl for `RpcData` or for
`Result`, because both are themselves `Serialize` — `E0119` on each. Specialization is unstable, so
the two candidates are separated by autoref depth instead:

```rust
impl<T: IntoOutput<Rpc>> Answered  for &&Answers<T>      // tried first
impl<T: Serialize>       Serialized for &Answers<T>      // reached only if the first does not apply
```

The macro emits `(&&Answers(value)).ulo_answer()`. A type that says what it is wins; anything else
is serialized. This keeps a handler's freedom to return a plain DTO while `RpcData` still means
`RpcData` — serializing an `RpcData` would wrap it in its own enum tag, since it is externally
tagged, and produce `{"Text": "hi"}` where the handler said `Text("hi")`.

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

**Breaking, in two shapes, and the compiler names one of them.** A handler written
`-> impl IntoResponse` that returns a `Result` no longer compiles, because `Result` is not
`IntoResponse` any more; it is written `-> Result<T, E>` or `-> impl IntoOutput<Http>`. A handler
whose return type is an alias for a `Result` compiles unchanged and now reaches the error chain
where it rendered its own error, with no diagnostic. Both were forms that skipped the chain.

## Roads not taken

**Drop RPC's serialize fallback.** It removes the coherence problem by removing the feature: every
handler would name `RpcData` or build the count itself. The fallback is what lets an RPC handler
return a domain type the way an HTTP one returns `Json`, and it is documented as the idiom.

**Keep `IntoResponse for Result` and have the macro route around it.** The macro would go on
reading the return type, which is the thing being removed, and the blanket would stay reachable by
anyone calling `into_response` directly.

**Remove `IntoResponse for HttpError`.** It is the one conversion that still renders an error,
reached only by a handler answering with an `HttpError` as its value rather than as its `Err`.
Removing it is a second break, on a different set of callers.

**Give gRPC an `IntoOutput<Grpc>`.** Nothing would implement it but the method's own message type,
and the impl could not be written generically, because which message is correct depends on which
method is being answered.
