# #120 — feat(axum): run the global chain before route matching

Merged 2026-07-17 into `master` from `feat/pre-routing-global-chain`, commit [`034625b`](https://github.com/ulo-rs/ulo/commit/034625bb8eee4e5b2be3521e56757ae90747e2df).

Anchors the global middleware chain before route resolution on the axum adapter. The chain previously ran inside each matched route's handler plus a 404 fallback, so requests axum answered itself — method mismatches (405), CORS preflight to routes without an `OPTIONS` handler — never reached global middleware, and middleware could not rewrite a request's path to change which route runs. A `GlobalChainService` now wraps the finished `Router` as a tower `Service` and runs the chain once per request, with the entire router as the routing step. Requests and responses cross the toni/native boundary by re-wrapping, no copying or buffering — the Send-only response body model (#117) lets axum's `!Sync` native body wrap directly into toni's `BoxBody`, so streaming responses (SSE) flow through the chain untouched.

- Adapter: `GlobalChainService` in toni-axum; route handlers reduce to adapt → handle → adapt; the fallback keeps the JSON 404 shape. Same-port WebSocket handshakes now traverse the chain like any other HTTP request, so middleware can reject upgrades.
- Contract docs: `AdapterContext::execute`, `HttpAdapter::into_lifecycle`, and the `Middleware` trait state one contract — the global chain runs pre-routing and sees every request; module-scoped middleware stays post-routing. (The trait doc previously said the opposite of `AdapterContext`.)
- Tests: `global_chain_conformance.rs` pins six behaviors every HTTP adapter must exhibit; the 405, preflight, and path-rewrite cases are the discriminators a post-routing anchor cannot pass. `TestServer::start_with` boots a pre-configured factory.
- ADR-0007 records the contract, the mechanism, and the rejected toni-owned-router alternative.

Deferred to follow-ups: re-anchoring poem, salvo, actix, and rocket against the conformance suite (rocket is the constrained one — fairings cannot short-circuit), and the core `CorsMiddleware` this unblocks.
