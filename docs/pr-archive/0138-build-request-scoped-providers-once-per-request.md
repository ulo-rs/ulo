# #138 — Build request-scoped providers once per request

Merged 2026-08-14 into `master` from `fix/request-scope-sharing`, commit [`b9c72a2`](https://github.com/ulo-rs/ulo/commit/b9c72a20dcbcf4e9927f1d16e26c21de43d5551c).

A request-scoped provider is now constructed once per request and shared by every injection site in it, instead of once per construction site.

Each site that builds providers — the enhancer factories, the controller, a `#[new]` constructor — minted its own instance cache, so a provider injected into both a guard and the controller was built twice in one request and the two sites held unrelated instances. The scope declares one instance per request; only the sites that happened to share a builder got one.

**Core.** `RequestCache::install` attaches a cache to the request parts at the head of the route pipeline; `RequestCache::adopt` picks it up. The cache travels on the parts because every construction site already receives them, so nothing in the provider SPI changes and a site holding only a clone of the parts still reaches the same cache.

**Macros.** The three generated construction sites adopt the request's cache rather than minting one.

**GraphQL.** Both controllers adopt from the parts they already split.

**Application context.** `resolve` and `resolve_by_token` adopt too, so a caller can install a cache on their own parts and place several resolutions in one scope — the grouping a CLI entry point or a test needs to exercise a provider tree the way a request would.

**Docs.** The type's existing claims — one cache per request, one instance across injection sites — describe the behaviour this fixes and needed no revision. Added: instances leave the cache by clone and injected fields bind owned values, so the guarantee is one *construction*, not one live value. A request-scoped provider whose state must be visible across sites has to hold it behind a shared handle.

**Tests.** An integration test covers the two paths built separately — an enhancer factory, resolved before the context exists, and a request-scoped controller built from the parts — asserting one construction for the first request and a distinct one for the next. It fails with `left: 2` if the install is removed. Unit tests cover adoption through a parts clone and the detached fallback.
