# #145 — Warn when a handler sets a response on the context

Merged 2026-08-15 into `master` from `feat/warn-handler-set-response`, commit [`ea279f8`](https://github.com/ulo-rs/ulo/commit/ea279f8b4b0a4d66a04fcd72a31fea92de59a062).

`ctx.set_response(...)` from a handler is discarded, and now says so.

A handler answers by returning, and the dispatcher writes what it returned over the context. So a response set from inside a handler never reaches the wire — legal to write, silently ignored, and exactly what someone arriving from a framework where the response object *is* how you reply will write first:

```rust
fn handler(&self, ctx: &mut HttpContext) -> Body {
    ctx.set_response(HttpResponse { status: 418, .. });   // discarded
    Body::text("hello")                                    // sent
}
```

They get 200 and no explanation. Now they also get:

```
WARN toni::injector::instance_wrapper: handler set a response on the context; the value it
     returned is sent instead. Return the response, or short-circuit from a guard or
     interceptor. method=GET path=/answer/both
```

**Behaviour is unchanged, and the call stays legal.** Setting a response is correct on the context — it is how guards and interceptors short-circuit, running while there is no response to overwrite. It just does nothing from a handler. A hard error would make this the only place in the framework where a documented method used in a legal position kills the request; the same register already covers request-scope auto-elevation, which warns and names the fix rather than refusing.

**Attribution is precise.** Only a response that appears across the handler call counts, so an enhancer that set one before the handler ran does not trip it.

**Tests.** The precedence was implicit in dispatch order with nothing holding it in place, so the test pins that: a handler sets a 418 and returns a 200 body, and the returned one goes out. The warning itself is not asserted — the suite has no log capture — so the test covers the rule rather than the message.
