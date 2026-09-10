# #181 — feat(factory)!: return startup failures instead of ending the process

Merged 2026-08-24 into `master` from `feat/create-reports-init-failure`, commit [`57d2bf6`](https://github.com/ulo-rs/ulo/commit/57d2bf66d8b66c80f0fad0131ddce4ed8610b209).

`ToniFactory::create`, `create_with`, `create_application_context` and `create_application_context_with` return `Result<_, StartupError>`.

One error class had three fates before, decided by nothing but which function it occurred in. `on_module_init` and `on_application_bootstrap` share a signature and the scanner wraps both failures into the same `HookFailed` variant, carrying module and hook name — yet a module-init failure panicked from `create`, a bootstrap failure returned from `bind`, and the same bootstrap failure was logged and swallowed by `create_application_context`, which handed back a context reporting success. That last one is the failure ADR-0024 refused for adapters, in a different function.

Nothing forced the panic. `create` is called from `main`, a test, or a spawned task, and all three can hold a `Result`. The constructors that genuinely cannot are the module-declaration ones: `imports: [ConfigModule::<T>::new(), SeaOrmModule::for_root(url)]` is a macro expression list where no `?` is available, and returning `Result` there would only move `.unwrap()` into the declaration. Those keep panicking, and ADR 0025 states that as the rule rather than leaving it as an inconsistency to be closed later.

`BindError` is renamed `StartupError`. It spans both startup phases now, and a `create` that binds nothing should not hand back a bind error. `Adapter` stays reachable only from `bind`.

### Components

- **toni** — the four entry points return `Result`; `initialize` returns `StartupError` rather than `anyhow::Error`, so a hook failure keeps the module and hook names instead of being flattened; the standalone context propagates its bootstrap-hook failure.
- **toni-cli** — the scaffolded `main` takes the result.
- **examples** — those with a `Result` main use `?`, which is the form a reader should copy; the ones spawning onto a `LocalSet` unwrap.
- **tests** — the three refusal tests read the diagnostic from the returned error instead of catching an unwind.
- **docs** — ADR 0025, plus the READMEs and adapter usage blocks.

### The boundary

`create` returning `Result` does not mean every failure inside it is returned. `ProviderFactory::build` returns the instance directly and has nowhere to put an error, so a database module that cannot reach its server still ends the process. Making provider construction fallible is deferred: it changes the SPI every integration and every macro-generated factory implements.

### Breaking

The four entry points return `Result`, and `BindError` is now `StartupError`. Matches on the type need renaming; the variants are unchanged.
