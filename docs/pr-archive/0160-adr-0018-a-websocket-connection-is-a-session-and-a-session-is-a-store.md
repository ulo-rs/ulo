# #160 — ADR 0018 — a WebSocket connection is a session, and a session is a store

Merged 2026-08-20 into `master` from `docs/adr-connection-is-a-session`, commit [`0c35ec7`](https://github.com/ulo-rs/ulo/commit/0c35ec7335d3f1d4466a250587e85a144f6c2183).

Records what connection-lifetime state on a WebSocket hangs on. ADR 0016 named the session and deferred it until something needed one; making the connect its own execution is what produced the need.

A connect guard's work now reaches `on_connect` and nothing after it, so authenticating once and acting on that identity per message still has nowhere to live. A gateway cannot hold it — one gateway serves every connection on its path and is refused a request-scoped dependency, which is the right answer rather than an obstacle.

The decision:

- A connection gets a **store, not a context** — a bag with no cache, no cancellation, no metadata, no phases. Modelling it as a second context type would make the confusion between the two lifetimes structural.
- Reached through `WsContext::session()` and a `Session` extractor, never as a field on `WsClient`. Two lifetimes get two access paths; a second field beside `extensions` is the arrangement that produced three bags across one connect.
- Its own type, because `Extensions` does not encode its lifetime.
- Created in `begin_connect` before the guards, dropped when the client is removed, with `Drop` as the teardown.
- `on_disconnect` gains a context, so teardown reads the session like everything else. It gains no enhancer chain: rejecting a disconnect is meaningless.
- No injectable and no session provider scope: every participant that would read it already holds the context.
- WebSocket only. `session()` is on `WsContext`, not on `HandlerContext` where it would answer `None` three times out of four.

Nest reaches connection scope by handing its client object to the same `getContextId` the HTTP router hands a request — same method, and the only difference is that a client lives for the connection. The ADR quotes it and records why it is not copied: one mechanism cannot express both lifetimes, and the per-message boundary is worth more than the free connection state.

Five roads not taken are recorded with their reasons. Docs only; the implementation follows separately.
