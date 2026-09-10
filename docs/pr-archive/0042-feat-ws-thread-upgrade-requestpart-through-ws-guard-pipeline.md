# #42 — feat(ws): thread upgrade RequestPart through WS guard pipeline

Merged 2026-04-18 into `master` from `feat/ws-handshake-parts`, commit [`02e67f4`](https://github.com/ulo-rs/ulo/commit/02e67f4037e4879fc3d72536ace86fceadafe667).

Guards declared with `#[use_guards]` on a WebSocket gateway were silently
ignored: the gateway macro never processed enhancer attributes on the impl
block, so `get_guard_tokens()` always returned an empty vec.

Separately, there was no way for a guard to inspect the HTTP upgrade
handshake (headers, cookies, auth tokens) — the upgrade request parts were
discarded at the adapter boundary before reaching guard resolution.

This threads `http::request::Parts` from the Axum upgrade handler through
`WsConnectionCallbacks.connect` → `begin_connect` → `resolve_guards` →
`factory.create(Some(parts))`, so request-scoped deps resolve against the
real upgrade request at connect time. Per-message guard calls pass `None`
(upgrade parts are per-connection; the guard already ran on connect).

The correct guard pattern for multi-context use (HTTP + WS) reads from
`WsClient.handshake.headers` — populated from upgrade parts by
`create_client_from_parts` — so it works identically at connect time and
per-message without injecting `Request`.
