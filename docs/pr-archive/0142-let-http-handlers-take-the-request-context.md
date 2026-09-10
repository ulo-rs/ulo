# #142 — Let HTTP handlers take the request context

Merged 2026-08-14 into `master` from `feat/handler-context-param`, commit [`bfd1060`](https://github.com/ulo-rs/ulo/commit/bfd1060d0068a676ad80123bb5d658bb5d95d3e7).

An HTTP handler can now declare `&mut HttpContext` and receive it, the way an RPC handler already holds its context.

Until now the handler saw the request through extractors and nothing else. Route metadata had no route to it at all — it lives on the context, and the context stopped at the enhancers. The extension bag needed an extractor of its own to cross the same boundary. This is the first step of the uniform-handler direction: the context becomes something a handler asks for rather than something only enhancers hold.

```rust
#[get("/read")]
#[set_metadata(Role("reader"))]
fn read(&self, ctx: &mut HttpContext) -> Body {
    let role = ctx.route_metadata().and_then(|m| m.get::<Role>());
    let principal = ctx.extensions().get::<Principal>();
    // ...
}
```

**Exclusive, not shared.** `&HttpContext` does not compile here and `&mut HttpContext` does: the wrapper's future must be `Send`, `&T` is `Send` only where `T: Sync`, and `HttpContext` is deliberately not `Sync` — its body and response are unsync-boxed, which is what let the `Sync` bound come off `HandlerContext` in the first place.

That the reference is exclusive does not make the handler a second place to answer from. The dispatcher writes the handler's returned response onto the context after it completes, so a response set through the context there is overwritten. Short-circuiting remains with the enhancers, which run while there is no response yet.

**Scope.** `Route::execute` gains the parameter, the controller macro forwards it to any handler that declares it, and the four manual `Route` impls in the GraphQL crates take it and ignore it. Handlers that do not want the context are unchanged.

**Tests.** Route metadata is what the assertion reads, since it is the surface that had no other path to the handler; the guard's extension write is checked alongside it. A second case pins that the context sits beside ordinary extractors rather than replacing them.
