# #124 — feat(rocket): run the global chain before route matching

Merged 2026-07-17 into `master` from `feat/rocket-pre-routing-chain`, commit [`59ae385`](https://github.com/ulo-rs/ulo/commit/59ae385db46b9e36b677310af7d4b4d21f9a9b7d).

Anchors the global middleware chain before route resolution on the rocket adapter — the last of the five, and the internal-matching case ADR-0007 anticipated. 404s, 405s, and WebSocket handshakes now reach global middleware, and a middleware path rewrite changes which route runs. All five HTTP adapters now pass the conformance suite.

- Adapter: fairings cannot short-circuit with a response, so rocket offers no pre-routing anchor at all. One catch-all route per method hosts the chain; routing is internal (`match_route` over the toni route table, with param capture and 405/`Allow` semantics), and rocket's router reduces to connection serving.
- WebSocket: upgrades need the borrowed rocket request, which the `'static` routing closure cannot hold — the closure returns a marker response for WS paths, and the outer handler performs the upgrade only if the marker survived the chain, so middleware can reject upgrades by replacing the response.
- Tests: rocket instantiated in the conformance suite (30 tests across all five adapters).
- ADR-0007: rocket realization recorded; conformance status now reads all-five.

This closes the port series. Next up separately: the core `CorsMiddleware` the series unblocks.
