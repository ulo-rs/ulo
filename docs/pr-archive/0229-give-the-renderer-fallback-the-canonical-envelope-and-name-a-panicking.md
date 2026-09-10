# #229 — Give the renderer fallback the canonical envelope, and name a panicking handler

Merged 2026-09-01 into `master` from `fix/fallback-envelope-shape`, commit [`a7f3db2`](https://github.com/ulo-rs/ulo/commit/a7f3db277df0ba992e749ab6d6b206fd04608c08).

Two segments run below the error chain and reach no handler: a panicking `#[catch]` handler and a panicking renderer.

**The renderer's fallback carries the canonical envelope.** It answered with a bare `Internal Server Error` string. On HTTP that is a `text/plain` body where every other error is `{statusCode, message, error}`. On RPC and WebSocket a frame carries no content type, so the same string reaches the caller's decoder as a JSON string where every other error is an object, and a shape error there is indistinguishable from a corrupt frame. Each fallback is now the canonical envelope written as a string literal, which preserves the property the old one was built for: the renderer that just panicked was calling the error's own `kind()` and `message()`, and a literal calls neither.

**A chain panic names its handler.** The log carried the error and the panic message, so an operator with four registered handlers learned one was broken without learning which. It now carries the chain position, counted from the most specific handler — the order the chain runs.

The renderer keeps answering directly rather than re-entering the chain: the chain already declined this error, and a claim on a second pass would answer the request with "the renderer broke" instead of with the error the caller's action produced.

Tests: the three renderer-panic suites assert the envelope on their own transport. Each fails against the bare-string fallback — `Null` vs `"error"` on RPC, `text/plain` vs `application/json` on HTTP, and a decode error on WebSocket.

Depends on #228.
