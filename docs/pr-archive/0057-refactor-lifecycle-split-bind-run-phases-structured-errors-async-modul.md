# #57 — refactor(lifecycle): split bind/run phases, structured errors, async module hooks

Merged 2026-04-27 into `master` from `refactor/application-lifecycle`, commit [`d0d52d1`](https://github.com/ulo-rs/ulo/commit/d0d52d1069e8419148ea2ee8b798e0f9bf8f9b48).

## Summary

Four related improvements to the application lifecycle.

**bind/run split + ShutdownHandle**
- `ToniApplication::start()` previously conflated socket binding with serving. There was no way to observe the bound address before traffic started, forcing tests to use static port counters and fixed sleeps to discover which port the server landed on.
- Split into `bind()` + `run()`: `bind()` binds sockets eagerly and returns `BoundAdapters` (with the actual `SocketAddr`, including OS-assigned port `0`); `run()` drives serve loops and owns shutdown. `start()` remains a thin wrapper for the common case.
- `ShutdownHandle` (built on `event-listener`, no tokio dependency in core) replaces the `close()`-in-a-`select` pattern. Obtain it after `bind()`, hand it to a signal task, call `shutdown()` when ready; `completed().await` lets callers observe the full drain.
- `run()` and `close()` return `()` — shutdown is a committed transition, every hook runs regardless of whether earlier ones succeeded, and adapter close is firing a channel rather than a fallible operation.

**Structured error types** (`InitResult` + `BindError`)
- `InitResult = Result<(), Box<dyn Error + Send + Sync>>` replaces `anyhow::Result<()>` on all public startup hook signatures, removing the anyhow dependency from trait boundaries.
- `BindError` is a `thiserror` enum returned from `bind()`: `HookFailed { module, hook, source }` carries structured context; `Setup(Box<dyn Error + Send + Sync>)` covers internal init failures with a `From<anyhow::Error>` impl so `?` still works internally without leaking anyhow on the public surface.

**Async module hooks**
- `on_module_init` and `on_application_bootstrap` on `ModuleMetadata` are now `async fn` (with `#[async_trait(?Send)]` — `?Send` required because the container is `Rc<RefCell<...>>`).
- Shutdown hooks remain sync.

**Compile-time scope validation**
- Lifecycle hooks are only valid on singleton-scoped providers. Annotating any hook (`#[on_module_init]`, `#[on_application_bootstrap]`, `#[on_module_destroy]`, `#[before_application_shutdown]`, `#[on_application_shutdown]`) on a request- or transient-scoped provider is a compile error.
- Request-scoped: instances are created per-request and dropped after the response — they do not exist at application init or shutdown, so no hook can fire.
- Transient-scoped: lifetime is consumer-determined (singleton-shaped when consumed by a singleton, request-shaped otherwise), so hook behaviour depends on the consumer rather than the provider — rejected with an explanation rather than left ambiguous.
- Aligns with NestJS, which documents that lifecycle hooks do not fire for request-scoped classes.
