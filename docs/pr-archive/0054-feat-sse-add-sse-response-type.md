# #54 — feat(sse): add SSE response type

Merged 2026-04-23 into `master` from `feat/sse-response`, commit [`143183a`](https://github.com/ulo-rs/ulo/commit/143183afa3e2c8c9a015d0999e7da05754630442).

## Summary

- Adds `Sse<S>`, `SseEvent`, and `sse()` to `toni` — return from any handler
  to stream Server-Sent Events with the correct headers set automatically
- `SseEvent` builder covers all SSE fields: `data`, `id`, `event`, `retry_ms`;
  multi-line `data` is split correctly per spec
- `sse(stream)` convenience for infallible streams; `Sse::new(stream)` for
  fallible ones (`Stream<Item = Result<SseEvent, E>>`)
- Fixes `Bytes` extractor to accept any content-type (was incorrectly
  restricted to `application/octet-stream`), which is how `curl -d` broke it

## Four patterns covered in the example

| Pattern | How |
|---|---|
| Infinite poll | `stream::unfold` |
| Named event types | `SseEvent::data(...).event("name")` |
| Per-request push | `mpsc::channel` + `stream::unfold` |
| Service broadcaster | `broadcast::channel` — Rust equivalent of NestJS `Subject` + `asObservable` |

## Test plan

- [ ] `cargo test -p integration-tests --test integration sse` — 6 tests covering
  headers, wire format, event fields, multi-line data, fallible streams, and
  the broadcaster round-trip
- [ ] `cargo run --example sse`, then `curl -N http://127.0.0.1:3000/sse/live`
  in one terminal and `curl -X POST http://127.0.0.1:3000/sse/emit -d 'hello'`
  in another
