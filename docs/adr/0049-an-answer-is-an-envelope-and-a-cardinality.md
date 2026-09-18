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

**An answer is an envelope and a cardinality.** The type that carries the count is `Items`.

```rust
pub enum Items<T, E> {
    /// Nothing goes back.
    Empty,
    /// One item.
    One(T),
    /// Items until the stream ends. An `Err` item ends it.
    Many(BoxStream<'static, Result<T, E>>),
}
```

Each transport names its envelope and its item, and the cardinality is the same type where the
items are payload:

| Transport | envelope | item | item failure | carries `Items` |
| --- | --- | --- | --- | --- |
| RPC | — | `RpcData` | `RpcError` | yes |
| WebSocket | — | `WsMessage` | `Infallible` | yes |
| gRPC | metadata, extensions | erased | `GrpcStatus` | no — the payload is erased whole |
| HTTP | status, headers | `Frame<Bytes>` | `Box<dyn Error>` | no — a frame is data or trailers |

`RpcHandlerOutput` and `WsHandlerOutput` become names for `Items<RpcData, RpcError>` and
`Items<WsMessage, Infallible>`. One type, and a spelling each transport keeps for the signature a
hand-written handler writes: `ExecutionResult<WsHandlerOutput, WsError>` reads as a return type
where the instantiation spelled out does not.

**HTTP's body keeps `Option<Body>` over `Buffered | Streaming`.** It has the same three states, and
it is not a cardinality: an `http_body::Body` yields frames, and a frame is data or trailers, so a
stream of them is not a stream of payload items. `Items<Bytes, _>` drops the trailers a body
built from a tower service carries through `from_box_body`; axum hands those to hyper untouched,
and the other four adapters drop them in the adapter already. `Items<Frame<Bytes>, _>` keeps
them by making `Many` mean "frames, some of which are metadata", which is not what the variant says
anywhere else. The remaining reading puts trailers in the envelope, and neither
`HttpResponse` nor a `ReplyEnvelope` holds a value that arrives after the payload.

`Option<Body>` keeps apart two states one adapter renders differently. `None` is a response with no
body and an empty `Some` is a body with no bytes: actix answers the first with `finish()` and the
second with the content type the adapter defaults to `application/octet-stream`. Salvo's two
branches reach the wire identically, `ResBody::None` and an empty `Once` both reporting an ended
stream and an exact size of zero. Collapsing the states picks one of actix's two renderings, which
decides what a bodyless response is rather than substituting one spelling for another.

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

**`Transport` names the two halves separately.** `type Output` is what a transport answers with and
`type Error` is what it fails with, and `Answer<T>` is the `Result` over them, spelled as a free
alias because associated-type defaults are unstable. Bounding `Error` by `From<PanicRecovered>` is
what a shared walk needs to build a failure it did not get from a handler: the walk over HTTP's,
RPC's and WebSocket's interceptors catches a panic above the leaf and writes
`Err(T::Error::from(event))`.

The bound is not free: it fixes the `Err` half of every transport's answer to one shape at the trait,
which the adapter SPI does not otherwise require, and gRPC carries it for a walk its own chain does
not use. What it buys is that the shared walk writes that step itself. Without the bound the step is
a `fn interceptor_panicked` on the trait — one implementation per transport for a conversion the
shared code cannot spell in a type it does not know. HTTP, RPC and WebSocket lift any `ulo::Error`
through a blanket `From`; gRPC has none by ADR-0039 and names this conversion on its own.

## Consequences

Four things become writable that an unconstrained answer type does not allow.

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

All four wrappers end a tail on `None`. A stream or body that answers it is drained; one dropped
before that signals, and the producer behind it stops. An item or frame carrying an error is an
abnormal end: the transport stops drawing there and the wrapper is dropped un-drained, which reaches
the producer the same way. `ScopedBody` signals through its `FnOnce` where the other three cancel a
token, and WebSocket's item type is `Infallible`, so an errored item cannot arise there.

An RPC or WebSocket handler answers `Items::One` or `Items::Many` where the per-transport enums had
`Single` and `Stream`, and a WebSocket one builds its stream through `Items::stream` rather than
mapping each item into an `Ok` the wire cannot contradict. A gRPC
enhancer holding a `GrpcReply` reaches a header without a downcast and the message with one, where
both sit behind the method's own `tonic::Response<T>` otherwise. An HTTP handler writes what it
wrote before.

Two of the four transports carry `Items`, which is what an operator written over it reaches.
That is the limit of the type: an answer on HTTP is an `HttpResponse` and its body's count is two
levels below one, so an operator meets HTTP through code that knows about `HttpResponse` whatever
`Body` holds. The type does not unify the four, and unifying the two it does is what it is for.

## Roads not taken

**Erase a gRPC payload per item.** `Items<Box<dyn Any + Send>, GrpcStatus>` is the shape that
puts gRPC inside the cardinality rather than beside it, and it is what would extend `take`,
`timeout` and counting to a gRPC stream. It costs one heap allocation per streamed message where
tonic passes items by value, on the one transport whose streams are shaped for volume. The envelope
unifies without paying it, which is where the split above is drawn.

**Fold `HttpResponse` into `Items` whole.** A status and a set of headers are not a count of
items, and an answer that carried only a cardinality would have nowhere to put them.

**Fold HTTP's `Body` into `Items`, one level down.** It has the three states, and the reading
that makes it a cardinality puts trailers in the envelope, where nothing holds them. Carrying them
as items instead spends the meaning of `Many` to do it. What the fold would buy is a `Body` that
names its states, at the cost of the ones an `http_body::Body` already carries.

**Give a WebSocket item an error type to match RPC.** It would make one signature serve all four
without `Infallible`, and it would offer a handler a channel the protocol has no frame for. The
divergence is in the wire, and a type that hides it invites code that cannot run.

