# #194 — Reach a module from the app by type, or by name

Merged 2026-08-27 into `master` from `feat/module-lookup-by-type`, commit [`7cb5a80`](https://github.com/ulo-rs/ulo/commit/7cb5a80728a2ffe6b2ad4cebd5a2f0fc004330dd).

Providers resolve by type from anywhere, but the module scoping them was reachable only from inside itself, through an injected `ModuleRef`. `get_module::<M>()` on `ToniApplication` and `ToniApplicationContext` hands out the same handle from the outside, resolving providers in that module's scope with the same strict/global semantics.

The identity to match comes from `token_of::<M>()`: an exact match for a `#[module]` type or a generic library module (`ConfigModule<T>`), a unique-prefix match for an identity that folds in a config fingerprint (the GraphQL modules). `get_module_by_name` reaches modules whose identity is not a type — a `DynamicModule`'s base name. Both ambiguity cases return errors naming the colliding identities: two fingerprinted modules of one type, and one name shared by two configs. `ModuleRef` gains a `Debug` impl showing its module.

The `module_lookup` tests pin each path: static by type, generic by type, fingerprinted by unique prefix, dynamic by name, the two ambiguity errors, and that the handle enforces module scope (a root-module provider is not reachable through another module's handle).

Based on #193 — the fingerprinted-identity prefix match needs its identity format; retarget to master once it merges.
