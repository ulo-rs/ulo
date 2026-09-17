# 0049 — An answer is an envelope and a cardinality

Status: accepted

Draws the model the four answer types are instances of, and redraws the erasure boundary
[ADR-0048](0048-a-grpc-reply-travels-through-the-pipeline.md) put around a gRPC reply.

## Context

`Transport::Answer` names what an interceptor answers with and what an error handler claiming an
error answers with. It is unconstrained:

```rust
pub trait Transport: 'static {
    type Context: ExecutionContext;
    type Answer;
    const NAME: &'static str;
}
```

A function generic over `T: Transport` can carry an answer and can do nothing with it. `guards_for`,
`interceptors_for` and `claim` are generic for that reason — they move answers without reading one.
Everything that reads one is written four times.

The four instantiations:

| Transport | `Transport::Answer` |
| --- | --- |
| HTTP | `Result<HttpResponse, HttpError>` |
| RPC | `Result<RpcHandlerOutput, RpcError>` |
| WebSocket | `Result<WsHandlerOutput, WsError>` |
| gRPC | `Result<GrpcReply, GrpcStatus>` |

The `Err` side is settled: every way a call can fail leaves as `Err` carrying its cause, and the
chain runs once above the interceptors. The `Ok` side is four types, and three of them are one type
under three spellings.

`RpcHandlerOutput` and `WsHandlerOutput` are the same enum over different items — `Empty`, one, or a
stream. `HttpResponse.body` is `Option<Body>` over `Buffered(Bytes) | Streaming(BoxBody)`: no body,
one buffered body, a stream. That is the same three states written as a product of two types rather
than as a sum of three.

Two things vary that the shared structure does not name.

**The envelope.** HTTP carries a status and headers, gRPC carries metadata and extensions, RPC and
WebSocket carry neither. An envelope belongs to the answer and does not vary with how many items
follow it.

**Who owns the encoding.** On HTTP, RPC and WebSocket the framework encodes a handler's value into
`Bytes`, `RpcData` or `WsMessage` before an answer exists, so every enhancer below sees one item
type per transport. On gRPC tonic owns the codec and the item stays the method's own message to the
wire. That is what makes a gRPC reply's type per-method where the other three are per-transport, and
it is the constraint ADR-0048 erased around.

## Decision

**An answer is an envelope and a cardinality.**

```rust
pub enum Cardinality<T, E> {
    /// Nothing goes back.
    Empty,
    /// One item.
    One(T),
    /// Items until the stream ends. An `Err` item ends it.
    Many(BoxStream<'static, Result<T, E>>),
}
```

Each transport names its envelope and its item, and the cardinality is the same type:

| Transport | envelope | item | item failure |
| --- | --- | --- | --- |
| HTTP | status, headers | `Bytes` | `BodyError` |
| RPC | — | `RpcData` | `RpcError` |
| WebSocket | — | `WsMessage` | `Infallible` |
| gRPC | metadata, extensions | erased | `GrpcStatus` |

`RpcHandlerOutput` and `WsHandlerOutput` become names for `Cardinality<RpcData, RpcError>` and
`Cardinality<WsMessage, Infallible>`. One type, and a spelling each transport keeps for the
signature a hand-written handler writes: `ExecutionResult<WsHandlerOutput, WsError>` reads as a
return type where the instantiation spelled out does not.

HTTP's body carries a content type beside its bytes, so `Body` contains a cardinality rather than
being one, and `HttpResponse.body` sheds its `Option` once `Empty` is reachable inside it. That is
a wider change than the other two — five adapters read `Body`, and the content type has to reach
the headers or stay on it — and it is made on its own.

**A WebSocket item cannot fail, and `Infallible` is where that is written.** The protocol has no
out-of-band channel to report a failed item — an error to a client is another frame the gateway
shapes. Spelling the parameter `Infallible` states that in the type rather than in a comment beside
it, and `Result<WsMessage, Infallible>` is niche-optimised to `WsMessage`'s own layout.

**A gRPC envelope is uniform; a gRPC payload is not.** `ulo` names no tonic type: `GrpcStatus`
mirrors `tonic::Status`, and a request reaches core through `RequestCarrier`. The envelope arrives
the same way, as a trait core declares and the wire crate implements:

```rust
pub trait ReplyEnvelope: Send + 'static {
    fn header(&self, key: &str) -> Option<&str>;
    fn set_header(&mut self, key: &str, value: &str) -> Result<(), InvalidHeader>;
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
    fn as_any(&self) -> &(dyn Any + Send);
    fn as_any_mut(&mut self) -> &mut (dyn Any + Send);
    fn carries(&self) -> &'static str;
}

pub struct GrpcReply(Box<dyn ReplyEnvelope>);
```

`ulo-grpc` implements it on a newtype over `tonic::Response<T>`, which is what the orphan rule
leaves available and what `RequestCarrier`'s two carriers already do.

An interceptor stamping a reply header is written once for every method of every service, and reads
and writes one without naming the method's reply type or the wire crate. One that reads or replaces
the message downcasts, and that work is method-specific whatever the framework does.

Two shapes in the trait are decisions rather than plumbing. **Setting a header answers `Result`**: a
gRPC metadata key is a lowercase ASCII token and a non-`-bin` value is ASCII, and the alternative to
refusing one outside that is a panic inside tonic's own insert. **Borrowing the message is separate
from taking it**, because taking it consumes the carrier and the envelope with it. A downcast checks
`as_any` first and consumes only on a match, so a reply handed back on a mismatch still has its
headers.

## Consequences

Four things become writable that a `Transport::Answer` nothing can read does not allow.

One interceptor covering all four transports. `Guard<C>` is already written that way — it answers
`bool` — while `Interceptor<C, R>` has no shared vocabulary for `R`. A rate limiter, a timeout and a
metrics interceptor are one implementation each.

Operators over an answer — `timeout`, `take`, `tap`, `map_items` — as a module rather than as
per-transport code repeated at four seams.

One interceptor onion for three of the four. `ChainNext`, `RpcChainNext` and `WsChainNext` are one
structure over three type parameters: the interceptors left, what the chain wraps, and a `run` that
delegates back to a splitter. gRPC's pair is not, and cannot be folded in with them —
`LinkNext`/`LeafNext` are generic over a `FnOnce` delegate rather than holding a source, because the
delegate's reply type is the method's and does not appear in a shared signature.

One cancellation-scoping wrapper for the three stream wrappers. `ScopedBody` stays separate on two
counts: it implements `http_body::Body` where the others implement `futures::Stream`, and it fires
an opaque `FnOnce` rather than a token because `http` does not depend on `context`.

The three agree on what ends a tail, so the collapse inherits their rule rather than choosing one.
A stream is drained when it answers `None`; an item carrying an error is an abnormal end, and the
wrapper dropped un-drained fires the token so the producer behind it stops. `ScopedBody` counts an
errored frame as a drained body and does not fire. That disagreement outlives the collapse, because
the wrapper it belongs to is not in it.

An RPC or WebSocket handler naming `RpcHandlerOutput::Single` or `WsHandlerOutput::Stream` names
`Cardinality::One` or `Cardinality::Many`, and a WebSocket one builds its stream through
`Cardinality::stream` rather than mapping each item into an `Ok` the wire cannot contradict. An
HTTP handler answering `HttpResponse` with a `body` field spells that field differently. A gRPC
enhancer holding a `GrpcReply` reaches a header without a downcast and the message with one, where
today both are behind the method's own `tonic::Response<T>`.

## Roads not taken

**Erase a gRPC payload per item.** `Cardinality<Box<dyn Any + Send>, GrpcStatus>` is the shape that
puts gRPC inside the cardinality rather than beside it, and it is what would extend `take`,
`timeout` and counting to a gRPC stream. It costs one heap allocation per streamed message where
tonic passes items by value, on the one transport whose streams are shaped for volume. The envelope
unifies without paying it, which is where the split above is drawn.

**Fold `HttpResponse` into `Cardinality` whole.** A status and a set of headers are not a count of
items, and an answer that carried only a cardinality would have nowhere to put them. HTTP contains a
cardinality rather than being one, which is also true of gRPC and vacuously true of the other two.

**Give a WebSocket item an error type to match RPC.** It would make one signature serve all four
without `Infallible`, and it would offer a handler a channel the protocol has no frame for. The
divergence is in the wire, and a type that hides it invites code that cannot run.

**Constrain `Transport::Answer` to `Result<Answer<Self>, Self::Error>`.** The bound is writable and
would make every generic function see the structure without an associated-type projection at each
use. It fixes the `Err` half of every transport's answer to one shape at the trait, which the
adapter SPI does not otherwise require, and nothing in this decision needs it.
