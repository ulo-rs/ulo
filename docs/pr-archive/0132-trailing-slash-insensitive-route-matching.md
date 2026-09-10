# #132 — Trailing-slash-insensitive route matching

Merged 2026-07-29 into `master` from `feat/trailing-slash-insensitive-routing`, commit [`0d1c442`](https://github.com/ulo-rs/ulo/commit/0d1c442545028ca1dbe968c3e6c621adb47b9e93).

Routes now match regardless of trailing slash on all five HTTP adapters: `/app` and `/app/` address the same route, `#[get("/x/")]` registers the same route as `#[get("/x")]`, and the root `/` is untouched. Both sides normalize — `join_route` trims joined paths, and `AdapterContext::execute` trims the request path (query preserved) before the global chain runs, so middleware path checks and route matching see the same canonical form. The request is rewritten, not redirected. ADR-0011 records the decision.

- **core**: `join_route` result trimming, request-path normalization in `AdapterContext::execute`, same trim for same-port WebSocket registration paths and `for_route` patterns
- **toni-poem**: `{param}` segments are rewritten to poem's `:param` at mount time; the `/*toni_fallback` catch-all is removed — poem's radix tree prefers a catch-all child over a node's own exact data, so it shadowed any route registered at `/` — and the router's `NotFoundError` now maps to the toni 404 shape in the global-chain wrapper
- **toni-rocket**: internal `match_route` accepts `{param}` segments alongside `:param`
- **tests**: `trailing_slash_conformance.rs`, five cases per adapter (controller root, parameterized paths, query survival, slash-declared routes, root preservation); the poem and rocket fixes above are pre-existing gaps this suite exposed
- **docs**: ADR-0011 + index entry

Deferred to a follow-up: canonicalizing param syntax across adapters (`:id` vs `{id}`) — `{id}` now works everywhere, but `:id` is untranslated on actix and salvo; an independent inconsistency, not part of the slash semantics.
