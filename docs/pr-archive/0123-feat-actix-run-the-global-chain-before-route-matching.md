# #123 — feat(actix): run the global chain before route matching

Merged 2026-07-17 into `master` from `feat/actix-pre-routing-chain`, commit [`b8df24f`](https://github.com/ulo-rs/ulo/commit/b8df24fa844300116a9302a4ca1196b05099ada4).

Anchors the global middleware chain before route resolution on the actix adapter — the same re-anchor as axum (#120), poem (#121), and salvo (#122). 404s, 405s now reach global middleware, and a middleware path rewrite changes which route runs.

- Adapter: an App-level `Transform` middleware; routing happens inside the wrapped service, so the position is genuinely pre-routing. The distinctive constraint is `Send`: the chain requires a `Send` routing closure while actix's inner service is worker-local (`Rc`-based) — a oneshot channel bridge closes the gap, with the routing closure sending the request to and awaiting the response from a worker-local dispatch future joined alongside the chain. Mutations write back via `head_mut` + `match_info` refresh; no request clone may live across dispatch (actix's router mutates `match_info` through `Rc::get_mut`).
- 405 semantics: the fallback carries the same route-table logic as salvo — actix falls through to `default_service` on method mismatch, which previously answered 404 for both cases; it now answers 405 with an `Allow` header.
- Tests: actix instantiated in the conformance suite (24 tests across axum/poem/salvo/actix).
- ADR-0007: actix realization and conformance status recorded.

Deferred to follow-ups: the rocket re-anchor (fairings cannot short-circuit, so it may need internal matching), and the core `CorsMiddleware` once the substrate is uniform.
