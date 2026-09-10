# #116 — refactor(core)!: accept any ModuleMetadata as the root module

Merged 2026-07-15 into `master` from `refactor/drop-module-definition-enum`, commit [`3f9ee76`](https://github.com/ulo-rs/ulo/commit/3f9ee7658dcf3184ce6e2f72b4442577eb6e0825).

The factory entry points required `impl Into<ModuleDefinition>`, where `ModuleDefinition` was a single-variant enum transliterated from NestJS's `ModuleDefinition` union. That union's other members have no Rust future: `DynamicModule` already implements `ModuleMetadata` and fits in the existing `Box<dyn ModuleMetadata>`; forward references solve a JavaScript load-order problem that lazy macro-generated `imports()` bodies do not have; and async construction lives in `ProviderFactory::build`, one layer below the graph scan. The enum only narrowed the entry point — `DynamicModule` had no `From` impl, so a dynamic root module had to be hand-wrapped in the enum.

`create`, `create_with`, `create_application_context`, and `create_application_context_with` now take `impl ModuleMetadata + 'static`. Any type implementing the trait is a valid root, with no per-type registration.

- **Core** — entry points take `impl ModuleMetadata + 'static`; the scanner walks `Box<dyn ModuleMetadata>` directly; `ModuleDefinition` and the two hand-written `From` impls (`BuiltinModule`, `BroadcastModule`) are deleted; `ModuleMetadata` is re-exported at the crate root.
- **Macros** — `#[module]` no longer emits a `From<X> for ModuleDefinition` impl per module.
- **Tests** — adapter and integration test helpers take `impl ModuleMetadata`; the redis-broadcast test drops its enum-wrapping now that a `DynamicModule` root works directly.
- **Docs** — [ADR 0008](docs/adr/0008-root-module-is-any-modulemetadata.md) records the decision; a [companion explainer](docs/explainers/module-definition-and-the-nestjs-union.md) works through why each NestJS union member collapses in Rust. The ADR index is backfilled with 0004–0006, which were missing.

**Breaking change.** `toni::module_helpers::module_enum::ModuleDefinition` is removed; pass the module value directly. Ordinary call sites (`ToniFactory::create(AppModule)`) are unaffected.
