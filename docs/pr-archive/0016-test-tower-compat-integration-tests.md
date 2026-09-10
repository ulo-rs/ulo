# #16 — test: tower-compat integration tests

Merged 2026-03-27 into `master` from `feat/tower-compat-tests`, commit [`a492691`](https://github.com/ulo-rs/ulo/commit/a492691ffb328f362b3f34a04114d419eacd96d2).

## Summary

- Covers `toni_extensions()` — a toni middleware stamps a typed extension; a custom Tower layer reads it via the helper and echoes it as a response header. The escape hatch was documented but untested.
- Covers `ServiceBuilder` composition — two layers stacked via `ServiceBuilder::new().layer(A).layer(B).into_inner()` applied as a single `TowerLayer`, validating the documented idiomatic multi-layer pattern.
- Covers toni + Tower middleware interleaved in the same `configure_middleware`, confirming both run.
- Covers `CompressionLayer` — a body-transforming layer, not just a header-injecting one. This is the definitive proof that the `BoxBody` streaming path from `feat/response-streaming` works end-to-end for layers that rewrite response bytes.

## Test plan

- [ ] `cargo test --package integration-tests --test tower_compat`
