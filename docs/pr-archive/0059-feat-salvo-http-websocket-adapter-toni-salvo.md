# #59 — feat: salvo HTTP + WebSocket adapter (toni-salvo)

Merged 2026-05-01 into `master` from `feat/salvo-adapter`, commit [`9bb222a`](https://github.com/ulo-rs/ulo/commit/9bb222a7257101290de9d2dc1b6945659a49ac32).

## Summary

- Adds `toni-salvo`, a salvo-based adapter implementing `HttpAdapter` and `WebSocketAdapter` (same-port upgrade and separate-port servers), with streaming request and response bodies, graceful shutdown via `tokio::sync::watch`, and the standard toni 404 fallback.
- Fixes a core partition bug where a gateway with `port = 0` was misrouted as same-port whenever the HTTP server also requested `0` — comparing requested numbers conflated intent with coincidence.
- Fixes a `#[controller]` macro bug where aliased imports of body-consuming extractors (`use Bytes as Foo`) silently received `RequestBody::empty()` instead of the request body.

## What's in the branch

- **`toni-salvo` crate**: full HTTP + same-port WS + separate-port WS, request/response streaming end-to-end, graceful shutdown wired through a shared watch channel, transport errors surfaced via `tracing` rather than dropped.
- **Path conversion**: toni's `:param` rewritten to salvo's `{param}` at bind time, with unit tests covering trailing/leading/back-to-back parameters.
- **Method dispatch**: GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS map to salvo's `Router` filters; TRACE/CONNECT fall back to `Router::goal()` because salvo doesn't expose those filters — documented in the crate-level docs.
- **Core partition fix** (`toni/src/toni_application.rs`): same-port vs separate-port now keys off declaration intent, not literal port equality. Regression test in `integration-tests` declares both at `0` and asserts distinct OS-assigned listeners.
- **Macro fix** (`toni-macros`): when a handler has zero named body-consuming extractors and exactly one `Unknown`, the macro routes that `Unknown` through the body code path. Multiple custom parts-only extractors still coexist via the empty-body fallback. Behavioral test added to `integration-tests`.
- **Examples and docs**: `examples/salvo_poc.rs` exercises HTTP routes, path params, response streaming, both body extractors, and same/separate-port WebSockets. The crate-root docs cover usage, routing, methods, body streaming, WebSocket transport, graceful shutdown, and the bind-failure contract.

## Test plan

- [x] `cargo test -p toni-salvo` — 6 unit + 4 integration pass
- [x] `cargo test -p integration-tests --test integration` — 147 pass (146 prior + the new `port = 0` regression test)
- [x] `cargo doc -p toni-salvo --no-deps` builds cleanly
- [x] PoC manually exercised over the wire: HTTP, path params, streaming response (chunks land at 500 ms boundaries via raw socket timing), 5 MiB chunked POST through `BodyStream`, same-port WS echo on `:3001/chat`, separate-port WS echo on `:3002/ping`
- [ ] Reviewer eyeballs salvo version pin (`0.92`) and feature flag (`websocket`) for compatibility with their toolchain
- [ ] Reviewer confirms the macro fix doesn't break any of their custom extractors that rely on the previous Unknown-gets-empty-body behavior

## Notes

The toni doctest at `toni/src/traits_helpers/interceptor.rs:52` fails on this branch but also fails on master — pre-existing, unrelated to this PR.
