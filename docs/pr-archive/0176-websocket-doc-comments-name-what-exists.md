# #176 — WebSocket doc comments name what exists

Merged 2026-08-23 into `master` from `docs/ws-docs-name-what-exists`, commit [`647c9c1`](https://github.com/ulo-rs/ulo/commit/647c9c151d1d1fba96b4b0a5cf7c57b956fdcfa4).

Three doc comments in the WebSocket modules point at items that no longer exist, and one of them states the opposite of what the code does. Each is an unresolved intra-doc link that `cargo doc` reports and CI does not run.

- `WsContext` links `WsHandlerOutput` through `super::`, which is `context`. The type lives in `websocket`.
- The `WsClient` extractor points at `WsClient::extensions`, renamed to `session`, and calls it "the message's bag rather than the connection's". `session` is the connection's; the message's bag is `Extensions`. Rewritten to say which is which.
- `Session`'s example uses a bare `…` inside a ` ```rust,ignore ` block, which rustc still lexes and cannot tokenize.

With these, the only rustdoc warning left in `toni` is the stale `BodyExtractor` reference, which #175 removes along with the paragraph containing it.
