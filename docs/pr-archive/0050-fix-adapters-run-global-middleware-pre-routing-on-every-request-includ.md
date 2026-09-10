# #50 — fix(adapters): run global middleware pre-routing on every request including 404s

Merged 2026-04-23 into `master` from `feat/middleware-pre-routing`, commit [`3722147`](https://github.com/ulo-rs/ulo/commit/3722147b36a4d7fece0d1a40ec8ae78abb104b02).

Global middleware was applied after route matching, so unmatched
requests (404s) never ran it. CORS headers were missing on 404
responses; any global middleware was silently skipped for requests that
hit no route.

The framework now provides an `AdapterContext` at serve time; the
adapter calls `ctx.execute(req, routing_fn)` to run the global
middleware chain around its own dispatch — pre-routing, on every
request. Both adapters register a fallback so genuinely unmatched
routes also pass through global middleware before returning a 404.
