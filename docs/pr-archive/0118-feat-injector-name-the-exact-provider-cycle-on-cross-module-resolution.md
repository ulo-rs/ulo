# #118 — feat(injector): name the exact provider cycle on cross-module resolution failure

Merged 2026-07-17 into `master` from `feat/injector-cross-module-cycle-diagnostic`, commit [`41b8789`](https://github.com/ulo-rs/ulo/commit/41b878926a6cfdaa30a67f353829e9fc60682296).

A provider dependency cycle that spans two modules previously surfaced as `Cannot resolve dependencies for modules: [...]. Possible circular dependency or missing global provider.` — it named the modules rather than the providers at fault and conflated a cycle with a missing provider (a genuinely missing dependency already fails earlier, in `resolve_dependencies`). This makes the injector name the exact cycle.

The per-module topological sort sees only one module's providers, so a cross-module cycle escapes it and stalls the Phase-1 deferred-retry loop. At the stall the injector now builds a provider-token dependency graph across every module and searches it for a cycle, reporting the exact chain and each provider's module; when no cycle is found it falls back to listing each stuck module with its last deferral reason.

- Injector: `find_dependency_cycle` (DFS with a gray-set) in `dependency_graph.rs`; `build_provider_dependency_graph` and `diagnose_unresolved_modules` in `instance_loader.rs`, invoked at the stall site.
- Tests: unit tests for the cycle finder; a subprocess regression test (fixture binary) asserting the message, since the only public trigger (`ToniFactory::create*`) calls `process::exit` on init failure.

Deferred to a follow-up: a mutual module *import* cycle is silently dropped by the Kahn sort in `get_ordered_modules_token`, so those modules never reach Phase 1 and the app boots without them. That is a separate latent issue, left out of this change.
