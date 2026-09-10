# #114 — refactor(macros)!: pass modules directly, drop module_definition()

Merged 2026-07-14 into `master` from `refactor/pass-modules-directly`, commit [`be94ba5`](https://github.com/ulo-rs/ulo/commit/be94ba5ca8781eacb97bba5b2814e53bed1ecd4a).

Removes the `module_definition()` method the `#[module]` macro generated on every module struct. The factory methods take `impl Into<ModuleDefinition>` and the macro already emits the matching `From` impl, so the module value itself is the entry-point API: `ToniFactory::create(AppModule).await`. The single-variant `ModuleDefinition` enum stays as internal plumbing but no user-facing surface promotes constructing it.

- **macros**: `#[module]` no longer generates `module_definition()`; the `From<Module> for ModuleDefinition` impl carries direct passing
- **call sites**: all examples, integration tests, adapter-crate tests, and rustdoc examples pass modules directly
- **test harnesses**: helpers that took `ModuleDefinition` by value (`TestServer::start`, the `start_rpc_server`/`start_app` family, salvo/rocket/poem harnesses) widen to `impl Into<ModuleDefinition> + 'static`
- **CLI template**: `toni new` output passes `AppModule` directly
- **READMEs**: examples updated to the current API — several still showed a two-argument `create(module_def, adapter)` with `app.listen(...)`, the removed inline-struct macro forms, `#[guard(grpc)]`-style role markers, `Ok(None)` WS returns, and `HttpRequest` context builders

Verified with `cargo build --workspace --all-targets` and `cargo test --workspace` (all green).
