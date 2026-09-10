# #127 — refactor(middleware): relax the chain seam to FnOnce + Send

Merged 2026-07-21 into `master` from `refactor/middleware-fnonce-relaxation`, commit [`83c323c`](https://github.com/ulo-rs/ulo/commit/83c323cff06823ee74297b9a39fc162c8fb00d8e).

Relaxes the middleware chain's closure bounds from `Fn + Send + Sync` to `FnOnce + Send`. The chain is consume-once by construction — `NextInternal::run_internal` takes `Box<Self>`, so the final handler runs at most once — but the seam demanded re-callable, shareable closures: capabilities nothing used. `FinalHandler` now stores `Box<dyn FnOnce + Send>`, `MiddlewareChain::execute` and `AdapterContext::execute` take `FnOnce + Send` routing closures, and `NextInternal` drops `Sync`. Every `Fn` closure satisfies the relaxed bound, so the change is source-compatible for middleware and callers.

The unused capability had a concrete cost in the adapters, all of it now deleted:

- **poem, salvo** — the request shell rode through the chain in a take-once `Arc<Mutex<Option<…>>>` to satisfy `Fn + Sync`, with an unreachable "request already consumed" 500 branch. The shell moves into the routing closure directly.
- **actix** — the oneshot channel ends were wrapped in the same Mutex pattern; they are captured directly now. The channel bridge itself remains — it answers actix's `!Send` worker-local service, not the seam.
- **axum, rocket, route-scoped caller** (`instance_wrapper`) — a per-call clone layer inside the closure existed only to satisfy `Fn`.

At-most-once `next` is the intended semantics: zero calls is a short-circuit, one is pass-through, and more than once is retry/fan-out — which cannot re-drive a consumed request body and belongs a level up (clone the request, re-run a fresh chain). Middleware instances themselves stay `Send + Sync` and re-entrant; only the per-request continuation is linear.

ADR-0007's poem/salvo phrasing is updated to match. Salvo's `CarriedBody` extension keeps its Mutex — `http::Extensions` requires `Clone`, which is unrelated to this seam.

Verified: 280 integration tests including the 30 global-chain conformance cases (6 behaviors × 5 adapters) pass.
