# #196 — ADR 0030 — one source builds every dispatch target, and enhancer tokens resolve at create

Merged 2026-08-28 into `master` from `docs/adr-one-source-builds-every-dispatch-target`, commit [`cfb637b`](https://github.com/ulo-rs/ulo/commit/cfb637b7acb5ad8f8c8aae918008b0b461636f5e).

Records the proposal that one machine builds every dispatch target on every transport, and that enhancer tokens resolve at create.

ADR-0017 unified where dispatch targets are declared and how they register, and left construction per transport. The three machines encode the same singleton-or-per-call fork with three singleton payload types, and the differences cost things the transports never asked for: HTTP downcasts `Arc<dyn Any>` on every request, RPC carries a `Clone` bound and a second `singleton` field to reach its own instance for lifecycle hooks, and HTTP's per-call arm is the one dispatch target absent from its own execution's cache. Enhancer tokens resolve at two moments — a storage artifact, not a data constraint — so a misdeclared token fails `create()` on an HTTP controller and `bind()` on an RPC controller or gRPC service.

The proposal:

- `DispatchSource<T>` in core — `Singleton(Arc<T>)` / `PerCall(provider)` — with one `instance()` resolution path; the caller supplies its transport's `ProviderContext` variant.
- The singleton payload is concrete on every transport: lifecycle hooks reach the instance one way, and the downcast and the `Clone` bound go.
- HTTP adopts the call-time fork and the provider path: one wrapper set per handler, and a request-scoped HTTP controller joins the execution cache.
- Enhancer tokens resolve at create; bind hands stored bundles to adapters.
- The call sites — `Route::execute`, `execute_handler`, the tonic delegate — stay per transport.

ADR-0017's construction section and its enhancer-timing consequence gain supersession pointers.

Four roads not taken are recorded with their reasons: an erased payload, a trait-object source, unifying the call sites, and registering with adapters at create.
