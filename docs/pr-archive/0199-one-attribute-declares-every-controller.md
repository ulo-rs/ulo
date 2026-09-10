# #199 — One attribute declares every controller

Merged 2026-08-28 into `master` from `feat/one-controller-attribute`, commit [`32678b2`](https://github.com/ulo-rs/ulo/commit/32678b2a264c320edc7dc2d9bc8ac81a67d68e0d).

Implements ADR-0031 (#198): `#[controller]` is the only struct attribute for dispatch targets, and the handler impl names the transport.

`#[rpc_controller]` and `#[grpc_service]` are removed outright — no migration errors, the crate has no users to migrate. A `DispatchBridge` replaces `RoutesBridge`: its blanket default answers `Dispatch::Http(Vec::new())`, so a controller with no handler impl stays valid and dispatches nothing, and each handler-impl macro shadows it with an inherent `__toni_dispatch` — `#[routes]` answers HTTP routes, `#[patterns]` RPC with the source companion (which moves here from the removed struct attribute, along with the `RpcControllerTrait` impl), `#[grpc_methods]` gRPC with the companion it already emits. Two handler impls on one struct collide on the duplicate inherent fn and fail to compile.

With the transport out of the struct attribute, the construction machine loses its per-transport configuration: `generate_dispatch_system` takes only the struct name, the elevation warning names `#[controller(scope = "request")]` on every transport, and the per-call provider builds through the struct's `__toni_build_from_deps` bridge uniformly.

A gRPC service becomes an ordinary declaration — `#[controller]` on a plain struct, a plain inherent impl whose `#[new]` and `#[on_*]` attributes expand on their own, `#[grpc_methods]` on the tonic trait impl. The inline-struct form dies with the attribute that carried it; no declaration form remains that takes a struct inline. A route prefix on a controller whose handlers are patterns or gRPC methods warns at startup.

A controller with no `#[patterns]` impl is no longer an RPC controller — it dispatches nothing. The self-sufficiency pin in `rpc_tcp` moves to the expressible form: an empty `#[patterns]` impl registers as an RPC controller with no patterns.

Components: the `DispatchBridge` in core (replacing `__route`), the macro collapse in `toni-macros`, the attribute sweep across examples, adapter-crate tests and integration tests, and the in-source docs that named the removed attributes.

Depends on #197.
