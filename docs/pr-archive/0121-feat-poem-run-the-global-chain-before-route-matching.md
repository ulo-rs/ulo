# #121 — feat(poem): run the global chain before route matching

Merged 2026-07-17 into `master` from `feat/poem-pre-routing-chain`, commit [`584f5a9`](https://github.com/ulo-rs/ulo/commit/584f5a97a348b15e16349bac455924ef0cd32477).

Anchors the global middleware chain before route resolution on the poem adapter — the same re-anchor as axum (#120), realized through poem's native extension point: a `GlobalChainEndpoint` wraps the finished `Route` and runs the chain once per request with the whole router as the routing step. 404s, 405s, and WebSocket handshakes now reach global middleware, and a middleware path rewrite changes which route runs.

- Adapter: one poem-specific mechanism — the WebSocket upgrade slot lives in the `Request`'s internal state, not http extensions, so the original request shell threads through the chain in a take-once slot and the chain's output (method, URI, headers, extensions, body) is written back onto it before routing. Router errors reach the chain via `into_response()`, which is how method mismatches surface as 405s. Per-route endpoints reduce to convert → handle → convert.
- Tests: the conformance suite's six cases move into shared functions; `conformance_suite!` instantiates them per adapter (axum and poem, 12 tests). `TestServer::start_adapter` is the parameterization point for the remaining adapters.
- ADR-0007: poem realization and conformance status recorded.

Deferred to follow-ups: the salvo and actix re-anchors, rocket last (fairings cannot short-circuit, so it may need internal matching), and the core `CorsMiddleware` once the substrate is uniform.
