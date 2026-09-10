# #119 — fix(injector): stop dropping import-cycle modules from instantiation

Merged 2026-07-17 into `master` from `fix/silent-import-cycle-drop`, commit [`d340041`](https://github.com/ulo-rs/ulo/commit/d3400411ad10be051cddc5c79e39a496e8464f06).

`get_ordered_modules_token` topologically sorts modules by their imports. On a mutual import cycle (`ModuleA imports ModuleB`, `ModuleB imports ModuleA`) no module is ever "ready", so the Kahn loop broke and returned a partial order that omitted the cyclic modules. Those modules never reached Phase 1, their providers were never instantiated, and the application booted without them — no error, no diagnostic.

An import edge is a visibility relationship, not a construction dependency, so a cycle is not inherently fatal. This appends the remaining modules to the order in a deterministic order instead of dropping them. The Phase-1 deferred-retry loop then resolves the real instantiation order: a harmless cycle boots with all providers, and a genuine provider cycle underneath surfaces as a precise error rather than silence.

- Injector: `get_ordered_modules_token` in `container.rs` appends the unvisited (cyclic) modules rather than breaking with a partial order.
- Tests: a mutual import cycle boots and both modules' providers resolve; a one-way provider dependency crossing the import cycle resolves through the deferred-retry loop.
