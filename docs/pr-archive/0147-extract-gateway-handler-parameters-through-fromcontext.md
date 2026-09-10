# #147 — Extract gateway handler parameters through FromContext

Merged 2026-08-15 into `master` from `feat/ws-extractors`, commit [`b1f3d9a`](https://github.com/ulo-rs/ulo/commit/b1f3d9ab9bf60757d7eebab4d354022c2d302b00).

A WebSocket handler now takes what it needs rather than the pair the dispatcher happened to pass.

## Before

```rust
async fn place(&self, client: WsClient, message: WsMessage) -> WsHandlerResult
```

Not a preference — the dispatcher called every handler with exactly those two, so each declared both whether or not it used either, parsed the frame by hand, and could ask for nothing else.

## Now

```rust
#[subscribe_message("place")]
async fn place(&self, Payload(order): Payload<PlaceOrder>) -> WsHandlerResult

#[subscribe_message("whoami")]
async fn whoami(&self, ext: Extensions, ctx: &mut WsContext, client: WsClient) -> WsHandlerResult

#[subscribe_message("ping")]
async fn ping(&self) -> WsHandlerResult
```

`WsClient`, `WsMessage` and `Extensions` are `FromContext<WsContext>`; `Payload<T>` deserialises the frame — JSON from text, JSON over bytes from binary, and a message pointing at `WsMessage` for ping, pong and close, which carry nothing to parse. `&mut WsContext` is passed through for a handler that wants the context itself.

**Existing handlers are unaffected.** `(WsClient, WsMessage)` classifies to exactly what it means today and keeps compiling — every such handler in the suite was left untouched and passes. The pair becomes the most common choice rather than the only signature.

## `handle_event` takes the context alone

Client, message and event all live on the context, so passing them beside it was duplication the extractors made visible. `GatewayTrait::handle_event` now takes only `&mut WsContext`, matching what `Route::execute` takes on the HTTP side.

## A gap this surfaced

WebSocket handlers can now read `route_metadata()` from their context, and it is always empty. `WsContext` carries the field and returns it, but **no WS macro handles `#[set_metadata]`** — not gateway-level, not per-handler — and `GatewayWrapper` passes a gateway-wide value that nothing populates.

Wiring that attribute for gateways is a separate feature and is not here. The test asserts the metadata is empty rather than reading a value from it, so the gap stays visible instead of being quietly absorbed.

## ADR 0015

Records the extraction design across the three transports it now covers: why extraction is a trait bound rather than a name table, why one generic trait rather than one per transport, why the second shorthand is a concrete impl instead of a blanket, and what is deliberately not uniform — `Payload<T>` is a WebSocket and RPC spelling, and gRPC is excluded because its signature is dictated by the tonic trait.
