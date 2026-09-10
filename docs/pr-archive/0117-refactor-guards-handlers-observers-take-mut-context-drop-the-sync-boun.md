# #117 — refactor!: guards/handlers/observers take &mut context; drop the Sync bound on HandlerContext

Merged 2026-07-16 into `master` from `feat/sync-bound-realignment`, commit [`c0ca052`](https://github.com/ulo-rs/ulo/commit/c0ca0520714b3db83bd5d8e4cad103f3c11da0d1).

Guards, error handlers, and error observers receive the per-request context by exclusive reference (`&mut C`) instead of a shared `&C`, which lets `HandlerContext` drop its `Sync` supertrait. That bound was the single root of a chain of downstream costs — a `Mutex` around the HTTP request-body slot, a `Sync` requirement on response streams, and buffering at the adapter boundary. Removing it clears all of them, and a guard can now attach a value to the context in `can_activate` and have the handler read it.

A `&C` held across an `#[async_trait]` await is `Send` only when `C: Sync`; a `&mut C` held across the same await is `Send` whenever `C: Send`. Switching the three context-borrowing enhancer methods to exclusive references removes the requirement at its source.

**Core traits**
- `Guard::can_activate`, `ErrorHandler::handle_error`, and `ErrorObserver::observe` take `&mut C` / `&mut dyn HandlerContext`. `HandlerContext` drops to `Send` alone. The enhancer trait objects keep `Send + Sync` — they are bootstrap-built machinery shared across the app, not per-request flow data, so their `Arc<dyn …>` storage is untouched.

**Response body**
- `BoxBody` becomes `UnsyncBoxBody`, and `Body::stream` and SSE drop their `+ Sync` bound, so a stream holding non-`Sync` state flows through without an `Arc<Mutex<…>>` wrapper.

**Contexts**
- `HttpContext`'s request-body slot and `WsContext`'s response slot lose their `Mutex` and become plain `Option`s set through `&mut self`.

**Adapters**
- salvo and poem route buffered responses through their native buffered variants (preserving `Content-Length`) and streaming responses through their `Send`-only stream constructors. axum, actix, and rocket accept the `Send`-only body unchanged.

**Macro**
- `#[catch(T)]` functions take `&mut Ctx`. A shared `&Ctx` there would reintroduce `C: Sync` through the awaited inner future.

**Tests**
- The 40 guard, 14 error-handler, and 6 observer impls across the suite move to the new signatures. `cargo test -p toni --lib` (55) and `cargo test -p integration-tests` (238) pass; salvo and poem are additionally exercised at runtime for both buffered and streaming responses.

Deferred to a follow-up: re-deriving the pre-routing global chain on top of this. With the `Sync` bound gone the chain no longer needs its response-smuggling workaround, so it is rebuilt clean there rather than ported as-is.
