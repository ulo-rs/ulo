# #161 — A WebSocket connection gets a session store

Merged 2026-08-20 into `master` from `feat/ws-connection-session`, commit [`ece24a3`](https://github.com/ulo-rs/ulo/commit/ece24a35a426a82ad2a2246a1fef8f2518435d75).

What a connect guard establishes is readable for the life of the connection. Implements ADR 0018 (#160).

An execution's bag empties with the execution, so authenticating once at connect and acting on that identity per message had nowhere to live. A gateway cannot hold it either — one gateway serves every connection on its path, and a per-connection value injected there would belong to whichever connection built it first.

`Session` is a store with a connection's lifetime: a bag, and none of what a context carries besides. A context is the object for one execution; this is not one, and modelling it as a second context type would put two context-shaped things on one transport.

**Reached through the execution.** `WsContext::session()`, and as a handler parameter:

```rust
#[subscribe_message("whoami")]
async fn whoami(&self, session: Session) -> WsHandlerResult { … }
```

Never a field on `WsClient`. A second bag beside the execution's, told apart by its name alone, is the arrangement that produced three bags across one connect.

**Its own type**, because `Extensions` does not encode its lifetime and two of them read identically in a signature.

**Lifetime.** Created in `begin_connect` before the guards, dropped when the client is removed, `Drop` as the teardown. A reconnect gets a new store — this is a connection's lifetime, not a user's.

**Disconnect becomes an execution**, so teardown reads the session the way every other participant does. No enhancers run there: a disconnect cannot be rejected. Connection hooks reach the context by declaring a second parameter, which is how they read a session the client does not carry.

No injectable and no session provider scope. Every participant that would read one already holds the context; a service below those is passed the value.

## Breaking

`GatewayTrait::on_disconnect` takes a context. `toni-async-graphql`'s gateway implements the trait directly and is updated.

## Tests

Three properties: a connect guard's write is read by every later message, the execution's own bag stays empty across them, and teardown sees what the connection held.

Each is falsified against the mechanism it covers — making the session per-execution fails all three, and sharing one across connections fails the separation test. That last test first passed against a deliberately broken build: it read the first connection before the second existed, measuring the guard's counter rather than the store. It now reads again afterwards.
