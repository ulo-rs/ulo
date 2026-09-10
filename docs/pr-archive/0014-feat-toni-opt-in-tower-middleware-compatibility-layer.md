# #14 — feat(toni): opt-in Tower middleware compatibility layer

Merged 2026-03-26 into `master` from `feat/tower-compat`, commit [`6cf9dd6`](https://github.com/ulo-rs/ulo/commit/6cf9dd63b51079e7d0fcbc7fb587299462f2cc47).

## Summary

- Adds `TowerLayer<L>` — wraps any `tower::Layer` as a toni `Middleware`, making the entire tower-http catalog (CORS, tracing, compression, rate limiting, timeouts) available to any adapter without per-adapter work
- Adds `toni_extensions()` — lets custom Tower layers read toni-typed extensions set by preceding toni middleware
- Gated behind the `tower-compat` feature flag; zero cost when unused

## How it works

The bridge converts between toni's `HttpRequest`/`HttpResponse` and `http::Request<Bytes>`/`http::Response<Bytes>` at the `Middleware` trait boundary. The adapter never sees Tower. Path params and toni extensions survive the round-trip via opaque wrapper types in `http::Extensions`.

## Known limitations

- Tower layers that call the inner service more than once (`retry`, `hedge`) will panic — those are client-side patterns; server middleware calls downstream exactly once
- Off-the-shelf Tower layers cannot read toni-typed extensions directly (use `toni_extensions()` for that in custom layers)

## Test plan

- [ ] `tower_layer_adds_response_header` — `SetResponseHeaderLayer` injects header visible after conversion
- [ ] `tower_layer_cors_permissive` — `CorsLayer::permissive()` adds `access-control-allow-origin: *`
- [ ] `tower_layer_request_body_round_trip` — JSON POST body survives `HttpRequest → http::Request → HttpRequest` intact
