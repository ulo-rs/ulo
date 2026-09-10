# #177 — Error types say what can happen, doc links reach what they name, and CI holds it

Merged 2026-08-23 into `master` from `refactor/errors-say-what-can-happen`, commit [`45946e4`](https://github.com/ulo-rs/ulo/commit/45946e436a3f53b8029c5733ad8c54de8b0da373).

`BodyStream` declares a failure it cannot produce, nine intra-doc links across the workspace point at items a reader cannot reach, and nothing in CI would have caught either.

`BodyStreamError` is never constructed. Both arms of the read yield a stream — a buffered body is wrapped rather than rejected — so the `Result` covered a branch that does not exist, and the message it would have printed, "missing request body", named the condition `take_body` already reports as `BodyAlreadyRead`. `BodyStream` now declares `BodyExtractionError<Infallible>`: the shape every body extractor has, saying that the only way it fails is the body already having gone. `BytesError::MissingBody` was dead for the same reason and goes with it.

The links were found by running the gate rather than by reading, which is why the list crosses crates nobody was working in:

- `toni` — `TowerLayer` unresolved. Visible only with `--all-features`, since `apply_tower` sits behind `tower-compat` and the doc is compiled out otherwise.
- `toni-macros` — `toni::Error` twice; the crate does not depend on `toni`, so it can only ever be prose. Four `unclosed HTML tag` from un-backticked `Path<T>`, `Query<T>`, `Json<T>`, `Validated<T>`.
- `toni-tcp`, `toni-udp` — both describe graceful shutdown as happening "on `RpcAdapter::close`". That method does not exist; the RPC SPI is `register_handlers` and `into_lifecycle`.
- `toni-redis-rpc` — links into the private `wire` module and to `RequestEnvelope`. Delinked rather than made public: a docs reader cannot follow either, and widening the API to satisfy a link is the wrong direction.
- `toni-terminus` — `HealthCheckService::check` from two modules that do not import the type.

A `docs` job runs `cargo doc --workspace --all-features --no-deps --locked` under `RUSTDOCFLAGS: -D warnings`. `--all-features` is load-bearing: a doc comment behind a feature gate is not linted while that feature is off, which is how `toni`'s own broken link survived a sweep that ran without it. The gate was checked against a deliberate broken link before being trusted — it fails on one and passes once reverted.

390 integration tests and 86 lib tests pass.
