# #61 — feat: rocket HTTP + same-port WebSocket adapter (toni-rocket)

Merged 2026-05-01 into `master` from `feat/rocket-adapter`, commit [`956aecf`](https://github.com/ulo-rs/ulo/commit/956aecff9bb1add67fcde1f227e603c10ee2afe1).

## Summary

Adds `toni-rocket`, a rocket 0.5 + rocket_ws adapter implementing `HttpAdapter` with same-port WebSocket upgrade. Two design notes worth flagging:

- Request bodies are buffered up to 32 MiB. Rocket's `Data<'r>` is lifetime-bound, so the streaming-body pattern from the other adapters can't escape the handler future. `BodyStream` still works — the full payload arrives as a single chunk.
- `WebSocketAdapter` is intentionally not implemented. Rocket and rocket_ws don't expose the primitives needed to host a separate-port WS server cleanly. Pair toni-rocket with toni-tungstenite when you need it.

## Test plan
- [x] `cargo test -p toni-rocket` — 3 integration tests pass
- [x] `cargo test -p integration-tests` — 147 tests pass (no regression)
- [x] `cargo doc -p toni-rocket --no-deps` builds clean
- [x] PoC exercised over the wire (routes, path params, response streaming verified at chunk boundaries, buffered + chunked POSTs, same-port WS, graceful shutdown)
- [ ] Reviewer eyeballs rocket version pin (`0.5`), rocket_ws (`0.1`), and the 32 MiB body limit constant
