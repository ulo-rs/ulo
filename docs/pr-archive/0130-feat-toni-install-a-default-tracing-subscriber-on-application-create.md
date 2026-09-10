# #130 — feat(toni): install a default tracing subscriber on application create

Merged 2026-07-27 into `master` from `feat/default-logger`, commit [`ba16726`](https://github.com/ulo-rs/ulo/commit/ba16726f845840cc57c1989e1090250eb56d2462).

Applications get framework logs out of the box: `ToniFactory::create` and `create_application_context` install a default `tracing` fmt subscriber — stderr, `RUST_LOG`-filtered with an `info` fallback — via `try_init()`, so a subscriber the application installs first always wins. `RUST_LOG=off` silences it at runtime; the new default-on `logger` feature gates the `tracing-subscriber` dependency for compiling it out. Without this, an application that configures no logging runs silent, and a failed initialization exits with code 1 and no output.

- **core**: `logger` feature + install at the top of `ToniFactory::initialize`, the funnel shared by both entry points; contract documented in a type-level `# Logging` section on `ToniFactory`, method docs link to it
- **dependent crates**: all 25 library crates declare `default-features = false` — feature unification would otherwise re-enable the logger and break the application's opt-out
- **examples**: subscriber boilerplate deleted; `logging.rs` now demonstrates the default and its escape hatches
- **docs**: ADR 0010 records the design (try_init back-off as the mechanism; no runtime knob)

Verified: workspace builds with and without default features; toni unit tests (67) and the integration suite (286) pass `--locked`; runtime-checked that an example with zero logging setup emits framework logs on stderr and that `RUST_LOG=off` yields none.
