# #179 — fix: startup and serve-loop failures unwind instead of exiting

Merged 2026-08-23 into `master` from `fix/startup-failures-unwind-instead-of-exiting`, commit [`4a154b8`](https://github.com/ulo-rs/ulo/commit/4a154b8d013a7a1dd298418fa2f8c96fdc84e55a).

Six `std::process::exit(1)` calls in library code are replaced: two in `ToniFactory` (`create_with`, `create_application_context_with`), two in `ConfigModule::new`, and one each in the axum and actix serve loops.

Exiting from a library skips every destructor, so a buffered log writer never flushes and the diagnostic that prompted the exit is the line most likely to be lost. It also removes the decision from an embedding process, and makes the failure untestable — an exit takes the test runner down with it, which is why three integration tests spawned fixture binaries and scraped their stderr. Those fixtures are deleted; the same diagnostics are asserted in process from the panic payload, and `ConfigModule::new` documents the panic it raises.

The two adapter exits fired after startup rather than during it. Removing them alone would trade one defect for its mirror image: an application that keeps serving the transports still alive while answering less than it advertises, with every liveness signal reading healthy. Each serve future signals shutdown when it returns, so a dead transport closes the application through the graceful path that already exists. During an ordinary shutdown the flag is already set and the signal is a no-op.

### Components

- **toni** — `create_with` and `create_application_context_with` panic on init failure, with `# Panics` sections naming what fails; `run` wraps each serve future so its return signals shutdown.
- **toni-config** — `ConfigModule::new` panics naming the config type; `from_env` remains the fallible path.
- **toni-axum, toni-actix** — serve errors log and return.
- **tests** — `common::panic_message` drives a future to a panic and returns its message; the circular-dependency, RPC-controller and gRPC-service refusals assert in process; a stub adapter covers a serve loop that dies; a missing required config value is covered.

Whether `ToniFactory::create` should be fallible rather than panicking is deferred: that is an API design change, not a fix.
