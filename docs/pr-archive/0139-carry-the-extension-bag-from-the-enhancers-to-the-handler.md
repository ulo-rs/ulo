# #139 — Carry the extension bag from the enhancers to the handler

Merged 2026-08-14 into `master` from `feat/extension-bus`, commit [`c816de1`](https://github.com/ulo-rs/ulo/commit/c816de1c8d01feb0989e000feb64604adbc65b26).

A value an enhancer attaches to `ctx.extensions()` now reaches the handler, on HTTP, WebSocket and RPC.

It did not before. The bag lived inside the context, and no handler receives the context — an HTTP handler gets extractors, a WebSocket handler gets a client and a message, a gRPC handler is a tonic trait impl. A guard that authenticated a caller had nowhere to put the principal, so the handler re-derived it. `guard_mut_context.rs` was accurate in calling this an enhancer-*to-enhancer* bus; the rustdoc on `Guard` and the framework notes claimed more than that.

**The bag.** `Extensions` becomes a shared handle rather than an owned map. Cloning it — or cloning the request parts it rides on — yields another view of the same storage, which is what carries a write across a pipeline boundary. It is created once per request at `AdapterContext::execute`, ahead of the global middleware chain, so a middleware writing before route resolution and a guard writing after it address the same bag.

**Reach, per transport.** HTTP handlers take `Extensions` as a parameter; the macro classifies it as a parts extractor. WebSocket handlers read `client.extensions` — the client the handler receives is a clone, so the context points it at the message's bag on the way through. RPC and gRPC handlers already hold the context.

**API change.** Mutation goes through `&self`, so `extensions_mut` is removed and `get` returns a clone rather than a reference, which is what a lock allows. `with` and `with_mut` cover payloads that cannot or should not be cloned. Four call sites and one example needed no more than the mechanical edit.

**WsClient.** Its own extension bag — a third store, read by nothing in the tree — is replaced by the message's. It is scoped to one message, which the field now says; a connection-lifetime bag is a separate question and does not exist yet.

**Not in scope.** gRPC handlers are generated around the tonic trait and cannot reach the context; that path is unchanged. `RpcContext::transport_extensions` stays as it is — it holds transport handles such as the originating NATS message, and is documented as distinct from the bus.

**Tests.** One integration file covers HTTP and WebSocket, including the pre-routing middleware write and the absence of leakage between requests; the RPC case sits with the other TCP RPC coverage where the transport helpers live. Unit tests pin that a context built outside the adapter seam still shares one bag with the request it yields, and that a bag installed upstream is adopted rather than replaced.
