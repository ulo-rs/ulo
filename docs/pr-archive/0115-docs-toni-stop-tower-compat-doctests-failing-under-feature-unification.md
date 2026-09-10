# #115 — docs(toni): stop tower-compat doctests failing under feature unification

Merged 2026-07-15 into `master` from `fix/apply-tower-doctests`, commit [`f74fc77`](https://github.com/ulo-rs/ulo/commit/f74fc779271e299af422166ef1596e03684b2075).

The `apply_tower` and `TowerLayer` examples were fenced `rust,no_run` but never compiled: each shows a free-standing `configure_middleware(&self, …)` body and imports `tower_http`, which is not a dependency. rustdoc skipped them until the `tower-compat` feature was enabled — which happens through feature unification whenever `toni` and `integration-tests` are tested together — at which point extraction and compilation failed the whole doc run.

Both are re-fenced `ignore`, matching every other `configure_middleware` example in the same files.

- `toni/src/traits_helpers/module_metadata.rs` — `MiddlewareConsumer::apply_tower` example
- `toni/src/tower_compat.rs` — `TowerLayer` example

Reproduce and verify: `cargo test -p toni --doc --features tower-compat`.
