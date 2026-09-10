# #78 — feat(core): AppError-driven error pipeline + panic recovery

Merged 2026-05-09 into `master` from `feat/app-error-redesign`, commit [`2960c51`](https://github.com/ulo-rs/ulo/commit/2960c51878b71e95b6f6862f3b6dcbdbdd77c45f).

Replaces the existing `Option<ErrorResponse>`-shaped error pipeline with
three orthogonal pieces — typed conversion, chain-of-responsibility
selection, fan-out observation — and catches handler/observer panics
across HTTP, RPC, and WS so one bad codepath can no longer tear down the
dispatcher.

## What's new

**Core trait surface**
- `AppError` — primary trait with `kind()`, `message()`, `details()`, plus
  per-transport rendering (`into_http_response` / `into_rpc_data` /
  `into_ws_message`) all defaulted from `kind()`. Users implement once,
  get correct shapes on every transport for free.
- `ErrorKind` — small framework-owned taxonomy that drives the defaults.
- `ErrorObserver` — universal fan-out hook (logging / metrics / Sentry).
  No response, no decision-making — just observation.
- `PanicRecovered { during: PipelineSegment }` and `Cancelled` — typed
  framework events for caught unwinds and client give-up signals.

**Macros**
- `#[derive(AppError)]` with `#[app_error(KIND)]` per-variant attributes.
- `#[catch(T)]` — runtime-selection escape hatch for per-error-type
  handlers. Generates the `ErrorHandler<C, R>` impl.

**Dispatcher**
- User handlers wrapped in `AssertUnwindSafe + catch_unwind` across HTTP,
  RPC, and WS. Caught unwinds become `PanicRecovered` and route through
  the same observer + chain + default-render path as any typed error.
- Each observer call individually wrapped in `catch_unwind`; a panicking
  observer is logged via tracing and dispatch continues to the next one.
- `ExecutionResult<R>` is the typed dispatcher return, generic over the
  transport's success type.

**Wire-shape contract**
- RPC wire-Err frame: framework couldn't *route* (PatternNotFound, guard
  rejection, pipe abort).
- RPC wire-Ok+envelope: handler ran. Includes user `AppError` returns
  *and* recovered panics — both go through `into_rpc_data`.
- WS panic emits `into_ws_message` as a text frame; connection stays
  open and siblings are unaffected.

## What's removed

- `ErrorResponse` enum (the `Http(...) | Rpc(...) | Ws(...)` wart that
  forced every handler to wrap one variant).
- `LoggingErrorHandler<H>` newtype-wrapper hack — logging is now an
  `ErrorObserver` registration.

## Tests

184/184 integration tests pass. New coverage:
- HTTP handler-panic rendering + observer-panic isolation
  (\`integration-tests/tests/integration/panic_recovery.rs\`)
- WS handler-panic envelope + connection survival
  (\`ws_panic_recovery.rs\`)
- RPC TCP/UDP handler-panic envelope + connection survival
  (in \`rpc_tcp.rs\`, \`rpc_udp.rs\`)
- \`AppError\` derive and \`ErrorObserver\` fan-out
  (\`app_error_derive.rs\`, \`error_observer.rs\`)

## Deferred

\`IntoResponse<C>\` as a separate generic trait — design called for it,
but the user-facing surface is identical to \`AppError\`'s per-transport
methods and the trait-gymnastics aren't worth it without GAT machinery
that isn't here yet. Collapsed into \`AppError\`.
