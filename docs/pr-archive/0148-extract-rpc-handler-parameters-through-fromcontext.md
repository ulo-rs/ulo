# #148 — Extract RPC handler parameters through FromContext

Merged 2026-08-15 into `master` from `feat/rpc-extractors`, commit [`d7c5af7`](https://github.com/ulo-rs/ulo/commit/d7c5af7e17edd2a65f29ef032aefa2485f0f4a32).

An RPC handler now takes what it needs rather than the pair the dispatcher happened to pass.

## Before

```rust
async fn place(&self, order: PlaceOrder, ctx: &mut RpcContext) -> Result<Order, RpcError>
```

The dispatcher called every handler with exactly those two, and the macro decided whether to deserialise by inspecting the first parameter. A handler wanting only the payload still declared a context it never read; one wanting the extension bag could not ask for it at all.

## Now

```rust
#[message_pattern("orders.place")]
async fn place(&self, Payload(order): Payload<PlaceOrder>) -> Result<Order, RpcError>

#[message_pattern("orders.audit")]
async fn audit(&self, ext: Extensions, ctx: &mut RpcContext, raw: RpcData) -> Result<String, RpcError>

#[message_pattern("orders.ping")]
async fn ping(&self) -> Result<String, RpcError>
```

`RpcData`, `Extensions` and `Payload<T>` are `FromContext<RpcContext>`; `&RpcContext` and `&mut RpcContext` pass through for a handler that wants the context itself.

**Existing handlers are unaffected.** A parameter of none of those types is the call's payload, deserialised into it — the convention RPC handlers have always used. It becomes one case among several rather than the only shape a handler can take, and every such handler in the suite was left untouched and passes.

## `Payload<T>` is shared with WebSocket

It moves to `toni::extractors`. Both transports name their message the same way; each supplies its own impl, since what a frame and a call carry differ and so do the ways they fail to parse. HTTP carries no impl for it — there the payload is the request body, and `Json<T>`, `Bytes`, `Body<T>` and `BodyStream` name it more precisely than one word could.

## No single-reader rule here

Unlike the HTTP body, which may be a stream and is therefore read once, `RpcData::parse` borrows. A call's data can be read as many times as a handler wants, so two payload parameters are legal and mean two views of the same call — no arity check is warranted.
