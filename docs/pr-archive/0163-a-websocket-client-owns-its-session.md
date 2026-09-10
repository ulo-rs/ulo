# #163 — A WebSocket client owns its session

Merged 2026-08-21 into `master` from `feat/client-owns-its-session`, commit [`d7992a0`](https://github.com/ulo-rs/ulo/commit/d7992a0d799f6bc809afb9df61e9a546bedcb1dc).

The session moves onto `WsClient`, and `WsClient::extensions` is removed. Implements ADR 0019 (#162).

`WsClient` exists once per connection and carried the *execution's* bag, repointed on every context construction:

```rust
client.extensions = shared.extensions.clone();
```

That assignment repoints a clone. The disagreement between clones it produced is what put three bags across one connect — a per-connection object holding per-execution state, with nothing keeping the two owners of that bag in step.

**The client is born with its session.** `WsClient::new` creates it, so a connection and the state scoped to it come into being together, and `WsContext::session()` reads it from there.

**The execution's bag keeps one owner.** `WsClient::extensions` is removed; a handler that wants it takes `Extensions` as a parameter, on WebSocket exactly as on the other three transports. `WsContext::new` mutates nothing it is given, so no clone can disagree with another about which bag is current.

**An accessor, not a public field.** The values inside stay writable through the handle; the handle does not. That makes `client.session = Session::new()` on a clone unwriteable, which is the shape of the defect being unwound.

The wrapper's client map holds `WsClient` again — the pair introduced alongside the session in #161 has nothing left to carry.

## Breaking

`WsClient::extensions` is gone. Three readers existed in this repository, all tests, and each now takes `Extensions` as a parameter or reads the context. One doc comment recommending the field is corrected.

## Tests

A new one pins what the move buys: reading the session off a handler's own `WsClient` and through the context answer alike, since every clone is the same connection. Falsified with a `Clone` that mints a fresh store per clone — three of the four session tests fail.

373 integration tests pass.
