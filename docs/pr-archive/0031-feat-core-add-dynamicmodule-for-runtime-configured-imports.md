# #31 — feat(core): add DynamicModule for runtime-configured imports

Merged 2026-04-12 into `master` from `feat/dynamic-modules`, commit [`b160cba`](https://github.com/ulo-rs/ulo/commit/b160cba3526c3526d5b17bf6ddf48024a540de10).

## Summary

Static #[module] definitions can't express providers whose configuration is only known at runtime (e.g. a database URL). Integration crates like toni-seaorm need to return a configured module from a factory function — that requires something that implements ModuleMetadata without the user having to write all seven methods by hand.

DynamicModule is that primitive. A builder collects providers and exports at construction time; the scanner consumes them once via the usual ModuleMetadata path with no changes to the scanner or injector.

Usage in an integration crate:

DynamicModule::builder("SeaOrmModule").provider(SeaOrmConnectionFactory::new(database_url)).export::<DatabaseConnection>().global().build()

Usage in an app module:

#[module(imports: [SeaOrmModule::for_root(DATABASE_URL)])] pub struct AppModule;

## Test plan

- [ ] Workspace compiles cleanly (cargo check --workspace)

- [ ] A module that imports a DynamicModule resolves its exported providers correctly

- [ ] .global() makes exported providers visible to modules that don't directly import it

- [ ] providers() called a second time returns None (drain-on-first-call contract)
