# #133 — Canonical {param} route parameter syntax

Merged 2026-07-29 into `master` from `feat/canonical-param-syntax`, commit [`3df4f29`](https://github.com/ulo-rs/ulo/commit/3df4f29e198173ba994d71b51b1c33237a73fad9).

`{param}` is the one route parameter syntax, on every HTTP adapter. Declaring a segment with a leading colon (`#[get("/users/:id")]`) is now a compile error naming the replacement — the same retirement path the inline-struct declaration forms took. Mid-segment colons remain legal literal path characters. ADR-0012 records the decision and the reasoning (URI-template/OpenAPI convention, axum 0.8's own migration, `:` being ambiguous with literal paths).

- **toni-macros**: `:param` rejection at the two path parse sites (controller prefix, verb/`#[sse]` sub-paths), with a spanned migration error
- **toni-actix / toni-salvo**: the 405 route-table fallbacks now recognize `{param}` segments — previously a wrong-method request to a `{param}` route answered 404 instead of 405
- **examples / adapter-crate tests / README**: mechanical `:param` → `{param}` migration (13 files)
- **tests**: `param_syntax_conformance.rs` — `{param}` extraction and 405-on-param-path per adapter
- **docs**: ADR-0012 + index entry

The adapter SPI keeps its existing `:param` leniency (translation on axum/salvo, matching on rocket/actix/salvo fallbacks) — raw `register_route` callers bypass the macros, and removing working leniency buys nothing.
