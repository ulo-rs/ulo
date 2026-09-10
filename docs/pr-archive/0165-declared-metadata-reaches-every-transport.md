# #165 — Declared metadata reaches every transport

Merged 2026-08-21 into `master` from `feat/metadata-on-every-transport`, commit [`456910b`](https://github.com/ulo-rs/ulo/commit/456910b970cd6dce606dca54c084643e33462994).

`#[set_metadata]` is collected from the impl block as well as the handler, and on all four transports. Implements the population half of ADR 0020 (#164); the rename follows separately.

Before this, the attribute was consumed by one macro file and read from `&method.attrs`. So WebSocket, RPC and gRPC populated nothing at all, and the class half of Nest's `[getHandler(), getClass()]` existed nowhere — including on HTTP.

**Why it matters more than it reads.** A universal guard compiles today:

```rust
impl<C: HandlerContext> Guard<C> for RolesGuard { … }
```

Registered on a WebSocket gateway it did not error and did not refuse. It read an empty map, found no requirement, and admitted every message. Population is the whole fix: "empty" now means "not annotated", which is the semantic Nest relies on.

## How precedence works

The impl block's entries are emitted before the handler's, and a later `insert` on the same type shadows an earlier one. That settles the same result Nest reaches by searching its targets in order — at expansion, with no lookup-time work.

## Per transport

- **HTTP** — the controller's entries join each route's.
- **RPC** — one merged map per pattern that declares its own, the controller's as the base for the rest; the wrapper picks by pattern at dispatch.
- **WebSocket** — the same, keyed by event. Connect and disconnect read the gateway's.
- **gRPC** — the macro generates each method body, so the merged map is baked in behind a `OnceLock` rather than looked up.

The collector moved to `shared/set_metadata.rs`, three transports needing the same two-level read.

## A limit worth naming

A gRPC handler cannot read declared metadata. Tonic dictates that signature and it never receives the context, so guards, interceptors and error handlers are the participants this serves there. The test reads it through a guard for that reason.

## Tests

Two-level overlay pinned on each transport: a handler inherits what the impl block declares, and a handler that declares its own shadows the matching type while keeping the rest.

Each is falsified by suppressing the collection it covers — every one collapses to `none/none`.

One stale comment corrected: `ws_extractors` asserted the map was empty because "`#[set_metadata]` is not wired for gateways", which is no longer why.

378 integration tests pass.
