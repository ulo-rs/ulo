# #60 — feat: poem HTTP + WebSocket adapter (toni-poem)

Merged 2026-05-01 into `master` from `feat/poem-adapter`, commit [`0cbe2d1`](https://github.com/ulo-rs/ulo/commit/0cbe2d18cf0d2c6432b815433dfe290dc85b0ea4).

## Summary

Adds `toni-poem`, a poem 3.1-based adapter implementing `HttpAdapter` and `WebSocketAdapter` (same-port upgrade and separate-port servers). Streaming end-to-end on both sides, native graceful shutdown via poem's `run_with_graceful_shutdown`, standard 9 HTTP methods including TRACE/CONNECT.

## Test plan
- [x] `cargo test -p toni-poem` — 4 integration tests pass
- [x] `cargo test -p integration-tests` — 147 tests pass (no regression)
- [x] `cargo doc -p toni-poem --no-deps` builds clean
- [x] PoC exercised over the wire (HTTP, path params, response streaming verified at chunk boundaries, 1MiB chunked POST, both WS modes, graceful shutdown)
- [ ] Reviewer eyeballs poem version pin (`3.1`) and `websocket` feature flag
