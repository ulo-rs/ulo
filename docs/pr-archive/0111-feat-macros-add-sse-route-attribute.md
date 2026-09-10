# #111 — feat(macros): add #[sse] route attribute

Merged 2026-06-21 into `master` from `feat/sse-attribute`, commit [`24f4fd4`](https://github.com/ulo-rs/ulo/commit/24f4fd4e16209a39df40168ed6beecb49f83c6df).

Adds `#[sse("/path")]` as a dedicated route attribute for Server-Sent Events handlers. Handlers return a bare stream; the macro detects the `Item` type and emits the correct SSE wrapper at expansion time.

Two return shapes are supported:

- `impl Stream<Item = SseEvent>` → `sse(stream)` (infallible)
- `impl Stream<Item = Result<SseEvent, E>>` → `Sse::new(stream)` (per-event fallible)

The attribute always forces GET — SSE is a subscribe operation and `EventSource` is GET-only. Setup failures before streaming starts belong in a guard or a `#[get]` handler returning `Result<impl IntoResponse, E>`; `#[sse]` does not accept `Result<Stream, E>`.

- **Macro** — `#[sse]` added to `toni-macros`; routes as GET, pre-wraps the handler return in `sse()` or `Sse::new()` based on the stream's `Item` type
- **Tests** — integration tests covering the infallible and per-event fallible variants
