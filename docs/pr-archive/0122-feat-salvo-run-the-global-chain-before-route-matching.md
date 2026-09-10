# #122 — feat(salvo): run the global chain before route matching

Merged 2026-07-17 into `master` from `feat/salvo-pre-routing-chain`, commit [`361a7b7`](https://github.com/ulo-rs/ulo/commit/361a7b7931e537fdb6c01c7cee750c0800216329).

Anchors the global middleware chain before route resolution on the salvo adapter — the same re-anchor as axum (#120) and poem (#121), realized through what salvo's API surface allows. 404s, 405s, and WebSocket handshakes now reach global middleware, and a middleware path rewrite changes which route runs.

- Adapter: `Server::serve` takes the concrete `salvo::Service`, so the chain anchors as the goal of a catch-all router and drives an inner `Service` through salvo's public `hyper_handler` entry, threading the owned request shell through a take-once slot (poem's pattern). `ReqBody::Boxed`'s inner type is crate-private, so the chain's request body rides to route handlers in a request extension rather than being reconstituted into the salvo request.
- 405 semantics: salvo's router cannot distinguish a method mismatch from an unmatched path (method and path are both opaque filters), so the fallback consults the route table and answers 405 with an `Allow` header — previously both cases answered 404.
- Tests: salvo instantiated in the conformance suite (18 tests across axum/poem/salvo).
- ADR-0007: salvo realization and conformance status recorded.

Deferred to follow-ups: the actix re-anchor, rocket last (fairings cannot short-circuit, so it may need internal matching), and the core `CorsMiddleware` once the substrate is uniform.
