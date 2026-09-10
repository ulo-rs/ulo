# #79 — feat(core): error-handling layer — toni::Error, per-transport rendering, #[catch], observers

Merged 2026-05-12 into `master` from `feat/transport-error-types`, commit [`454befb`](https://github.com/ulo-rs/ulo/commit/454befb4318be6b0d2dc2840131a3584995e138a).

Adds toni-rs's error-handling layer. Handlers return `Result<T, E>` where `E` implements `toni::Error` (or a transport's convenience type); the framework lifts the error into the active transport's error type and renders a canonical envelope, with overrides via `#[catch(T)]` chain handlers and a universal `ErrorObserver` fan-out hook. Handler and observer panics are caught and surfaced as `PanicRecovered`.

## Core

- `toni::Error` trait — `kind` / `message` / `details`. The contract: `std::error::Error` plus the metadata the pipeline needs (chain dispatch, observer routing, rendering). Re-exported at `toni::Error`; `#[derive(toni::Error)]` with `#[error_kind(KIND)]` per variant.
- `ErrorKind` — coarse, transport-independent taxonomy with a stable `name()`. Each transport's renderer maps it to that transport's wire form (`errors::http_status` / `http_reason` for HTTP; RPC and WS render from `name()`).
- Per-transport handler error types — `HttpError`, `RpcError`, `WsError`: named convenience variants plus an `AppError(Arc<dyn toni::Error>)` wrapper variant, a `From<E: toni::Error>` blanket so `?` works in handlers, and an inherent renderer (`to_response` / `to_data` / `to_message`). None implement `toni::Error` itself — the blanket and std's reflexive `From<T> for T` would collide.
- Framework events — `GuardRejection`, `MiddlewareFailure`, `PanicRecovered { during: PipelineSegment }`, `Cancelled` — implement `toni::Error` and flow through the same observer + chain pipeline as user errors.
- `ExecutionResult<R, E>` — the typed dispatcher return, generic over `(success type, transport error type)`.

## Override + observation

- `#[catch(T)]` — chain handler that produces the transport response directly and runs ahead of the default rendering. Registered per controller/route via `#[use_error_handlers(...)]` or globally. The override path for headers (`Retry-After`), domain-specific body shapes, and re-shaping framework events.
- `ErrorObserver` — universal fire-and-forget hook (logging, metrics, Sentry). Each observer call is `catch_unwind`-wrapped so one panicking observer doesn't break dispatch.

## Dispatcher

- Handler bodies wrapped in `catch_unwind` across HTTP / RPC / WS; recovered panics become `PanicRecovered` and flow through the normal observer + chain + fallback pipeline.
- RPC wire-shape contract: framework dispatch failures (`PatternNotFound`, guard rejection, pipe abort) surface as wire-Err frames (`{"err":{"status":..., "message":...}}`); handler errors surface as wire-Ok frames carrying the canonical envelope (`{"response":{"status":"error","kind":..., "message":...}}`). Clients branch on `err` (transport-layer problem) vs `response.status == "error"` (application-layer error). HTTP and WS don't make this structural distinction at the framing layer — both render the canonical envelope.

## Removed

- The old `ErrorResponse` enum and the `LoggingErrorHandler<H>` newtype-wrapper.

## Tests

184/184 integration tests pass. Coverage spans the derive macro (struct / enum / fallback kinds), observer fan-out, panic recovery on each transport, `#[catch]` against framework events and user errors, and the RPC wire-shape contract.

## Examples

`examples/error_handling.rs` — derived domain errors, a `#[catch]` override with a `Retry-After` header, `HttpError` as the convenience type, and framework-event reshaping.

## Deferred

A separate `ToResponse<C>` rendering trait was explored and dropped: a blanket impl can't coexist with per-type overrides without specialization, and `#[catch]` is the override path. `level()` / `Provenance` on the trait was rejected as over-generalization — the framework-vs-handler distinction only drives RPC wire framing, and it's already encoded in which `RpcError` variant the call site constructs.
