# #190 — Derive every DI token through one canonical function

Merged 2026-08-26 into `master` from `fix/unify-di-token-format`, commit [`3fbb157`](https://github.com/ulo-rs/ulo/commit/3fbb15758dd2c9c028fe5c40d23304635452696c).

A type's DI token is its fully-qualified `type_name`, base and generic parameters alike, produced by one function: `toni::di::token_of::<T>()`. Macros emit it with the full written type, every runtime lookup calls it, and no site string-formats a token. Implements ADR-0028.

Previously the token format depended on where it was built. Registration, `resolve`, factory-closure dependencies, and explicit `#[inject(Type)]` used the full `type_name` path; a bare `#[inject]` on a generic written type produced a bare-base string (`ConfigService<my_crate::AppConfig>`) that only hand-maintained `format!` calls in toni-config and core's `Extension<T>` matched. The formats met nowhere:

- `#[inject] pool: Pool<Postgres>` aborted startup against toni-sqlx's registration, while the `PgPool` alias worked — the written spelling decided the outcome.
- `resolve::<ConfigService<T>>()` and `ModuleRef::get` could not find toni-config's registration.
- A `provider_factory!` closure depending on a generic type found neither format.
- `#[inject(ConfigService<T>)]` and bare `#[inject]` on the same field produced different tokens.
- A qualified field type (`my_mod::Type`) failed to compile — the emitted lookup used the last path segment only — or silently took the wrong token when a same-named type was in scope.
- `provider_value!(Handle<Marker>, …)` was a proc-macro panic: the generated struct name kept the `<` and `>`.

**Core** — `token_of` in `toni::di`; `Extension<T>`, `Extensions`, `Request`, `ModuleRef`, broadcast, and the app-context lookups all derive through it. `ConfigModule<T>::get_id` moves to the same key, matching how `#[module]` identities are already formed, so a lookup-modules-by-type could not reproduce the mismatch in the module namespace. `GraphQLModule`'s identity keeps its hand-built form: it deliberately excludes the context-builder parameter, and `token_of::<Self>()` would change which modules dedup.

**Macros** — `extract_type_token` collapses to a single `token_of` emission with the full written type; provider registration, exports, multi-provider synthetic tokens, and enhancer tokens emit the same call; generated provider struct names are sanitized to identifier characters.

**Integration crates** — toni-config, toni-sqlx, toni-diesel, toni-mongodb, toni-seaorm, toni-redis, toni-prisma, toni-redis-broadcast, and toni-terminus register through `token_of` instead of direct `type_name` calls or hand-built strings.

**Token module consolidation** — `IntoToken` moves into `di::token` beside `Token` and `token_of`; the unused `TypeToken` marker is removed; `Token` consts implement `IntoToken`, so `get_by_token`/`resolve_by_token` accept them directly; the scanner matches global-enhancer registrations against the `APP_GUARD`/`APP_INTERCEPTOR` consts instead of duplicate string literals.

**Tests** — `integration-tests/tests/integration/token_format.rs` pins one registration against every lookup path. Against the prior code, three of its tests fail at startup with the full-path/bare-base mismatch and two fail to compile (the last-segment emission and the generic `provider_value!` name).

Breaking: a string token hand-written in the old generic format (`#[inject("ConfigService<my_crate::AppConfig>")]`) no longer matches; the failure is a startup "dependency not found" naming both tokens.
