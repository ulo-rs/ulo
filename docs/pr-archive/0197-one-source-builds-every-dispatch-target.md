# #197 — One source builds every dispatch target

Merged 2026-08-28 into `master` from `feat/one-source-builds-dispatch-targets`, commit [`675c211`](https://github.com/ulo-rs/ulo/commit/675c2110e2ced5aa96219c55d207ee7d5e5ac6ce).

Implements ADR-0030 (#196): one machine builds every dispatch target, and enhancer tokens resolve at create on every transport.

`DispatchSource<T>` in core carries the singleton-or-per-call fork — `Singleton(Arc<T>)` shared by every call, or `PerCall` resolving through the target's own provider, execution-cached, with init/bootstrap fired at the provider's build site, where hook resolution sees the concrete type. One macro generator (`generate_dispatch_system`) emits the per-call provider, the `Controller` object, the factory with its elevation scan, and the accessor; a transport supplies its object name, dependency-token list, build expressions, elevation warning and `Dispatch` variant. The source traits stay per transport, on thin generated newtypes over `DispatchSource<Struct>` — the orphan rule bars implementing a toni trait on the foreign generic directly in a consumer crate.

Per transport:

- HTTP: the two generated wrapper sets per handler — a singleton set downcasting `Arc<dyn Any>` on every request, a per-call set calling the construction bridge directly — collapse into one that resolves through the source at call time. A request-scoped HTTP controller joins the execution cache it was absent from. `ControllerInstance` is removed and `__toni_routes` takes the source.
- RPC: the singleton payload becomes the concrete `Arc<T>`, deleting the side-carried `singleton` field and the boxed copy; `RpcControllerSource::instance` answers `Arc<dyn RpcControllerTrait>`.
- gRPC: keeps its concrete payload; the source enum and its inherent `instance()` move to the shared type.
- All three: the macros stop deriving `Clone` — nothing clones a dispatch target — so non-`Clone` fields are legal in one.

Enhancer tokens resolve in DI phase 4 on every transport: the container stores the resolved `RpcControllerWrapper` and gRPC `(source, enhancers)` bundle, and bind only hands them to adapters. A misdeclared `#[use_guards(...)]` token fails `create()` where it failed `bind()` on RPC and gRPC. Two tests pin the phase, and both fail against bind-time resolution.

Injecting a dispatch target into an `#[injectable]` now stops at compile time — the injectable's derived `Clone` requires the field type `Clone` — with a message about the derive rather than the injection. The two injection-refusal tests implement `Clone` manually to keep the runtime not-found refusal covered.
