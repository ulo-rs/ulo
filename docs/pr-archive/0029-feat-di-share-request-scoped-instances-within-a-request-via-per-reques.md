# #29 — feat(di): share request-scoped instances within a request via per-request cache

Merged 2026-04-10 into `master` from `feat/request-scoped-cache`, commit [`e8fa6ce`](https://github.com/ulo-rs/ulo/commit/e8fa6cecbaec88e334466f4a5a3338535dcd18db).

## Summary

Request-scoped providers were behaving like transients — each injection point
got its own instance, even within the same request. Two services injecting the
same request-scoped type would get separate objects with identical values but
no shared identity.

This is the Rust equivalent of what NestJS solves with context IDs, implemented
without a global registry: a `RequestCache` travels inside `ProviderContext::Http`
for the lifetime of one handler invocation. Request-scoped providers check the
cache first; on a miss they construct and store; on a hit they return a clone.
The cache is dropped when the handler returns — no cleanup protocol needed.

`ToniApplicationContext` and `ToniApplication` gain `resolve()` and
`resolve_by_token()` for resolving request-scoped providers outside an HTTP
handler, which is the correct tool for testing and CLI use cases.

## Test plan

- [ ] `cargo test` — 121 passed, 0 failed
- [ ] `cargo build --examples` — clean
- [ ] Four previously failing DI validation tests now pass using `resolve()` with a synthetic `RequestPart`
