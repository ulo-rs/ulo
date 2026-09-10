# #125 — feat(toni): add CorsMiddleware for the global chain

Merged 2026-07-17 into `master` from `feat/cors-middleware`, commit [`4100b7d`](https://github.com/ulo-rs/ulo/commit/4100b7daf8da89c1f39eadc0b19de83a1ad74882).

Adds `CorsMiddleware` to core — one CORS implementation registered via `use_global_middleware`, working identically on all five HTTP adapters. The alternative, an `enableCors()` knob on the adapter trait, would mean five framework-specific integrations for one behavior the global chain expresses once (the same reasoning that kept `with_max_inflight` off `HttpAdapter`).

Preflight correctness rests on the pre-routing contract (ADR-0007, #120–#124): an `OPTIONS` preflight to a route without an `OPTIONS` handler reaches the middleware before the router would answer 405. The integration suite exercises exactly that shape.

- Core: `CorsMiddleware` + `CorsOptions` + `AllowedOrigins` in `toni::middleware` — origin allowlisting, credentials (with origin echo; the spec forbids `*` with credentials), preflight method/header advertisement with reflection when unconfigured, exposed headers, max-age, `Vary` correctness. Requests without an `Origin` pass through untouched; disallowed origins get no CORS headers and the browser enforces the block.
- Tests: six end-to-end cases including preflight-on-GET-only-route, allowlist echo with credentials, and disallowed-origin behavior.
