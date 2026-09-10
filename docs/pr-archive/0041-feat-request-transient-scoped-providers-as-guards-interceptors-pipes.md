# #41 — feat: request/transient-scoped providers as guards, interceptors, pipes

Merged 2026-04-18 into `master` from `feat/request-scoped-enhancers`, commit [`bd95920`](https://github.com/ulo-rs/ulo/commit/bd959208fd57e2cc95b82d70822c3f3da1b93d90).

## Summary

- Request- and transient-scoped providers silently emitted no enhancer roles, so they could never participate in the guard/interceptor/pipe pipeline regardless of `#[guard]` / `#[interceptor]` / `#[pipe]` markers — diverging from NestJS where scope does not block enhancer participation
- Introduces `GuardEntry { Ready, Factory }`, `InterceptorEntry`, and `PipeEntry` with the same split, plus `DynGuardFactory` / `DynInterceptorFactory` / `DynPipeFactory` traits for per-request construction
- The macro emits a factory implementor for request/transient providers; at request time the wrapper calls `create(request_parts)`, resolving singleton deps with `ProviderContext::None` and request-scoped deps with `ProviderContext::Http` — the same path controllers use
- RPC controllers validate factory guards at startup (`requires_http_parts()`) rather than deferring to first message; WS gateways apply the same startup check (forwarding upgrade `RequestPart` through the WS pipeline is a follow-up)
- `provider_factory!` at non-singleton scope with enhancer flags now correctly emits factory roles instead of silently dropping them

## Test plan

- [ ] `cargo test -p integration-tests` — 125 tests pass (122 existing + 3 new)
- [ ] `scoped_enhancers::request_scoped_guard_activates` — request-scoped guard, no deps, blocks/passes based on header
- [ ] `scoped_enhancers::request_scoped_guard_injects_request` — request-scoped guard injecting `Request`, reads header value
- [ ] `scoped_enhancers::transient_scoped_interceptor` — transient interceptor sets response header, verified per-request
