# #156 — ADR 0017 — a controller is a dispatch target, whatever the transport

Merged 2026-08-19 into `master` from `docs/adr-controller-is-a-dispatch-target`, commit [`b5ad223`](https://github.com/ulo-rs/ulo/commit/b5ad2231fb19d58762fec016dbf0dcbc55e74707).

Records the decision that a module's `controllers:` list holds dispatch targets on every transport, and `providers:` holds only what something may hold.

ADR-0016 made an RPC controller build per call and stop being injectable, and the same reasoning was applied to gRPC services. Both are still declared under `providers:` while the injector refuses to resolve them, so the declaration states a property the framework denies. The distinction exists only at runtime, in `dispatch_target_tokens` and `Module::dispatch_targets`.

The decision:

- `#[controller]`, `#[rpc_controller]` and `#[grpc_service]` are declared in `controllers:`. `#[websocket_gateway]` stays in `providers:` because a gateway is held, to broadcast from elsewhere — resolvability is the criterion rather than the transport.
- `Controller` hands over a `Dispatch` — a closed enum carrying HTTP routes, an `RpcControllerSource`, or a `GrpcServiceSource` — instead of a route list.
- `Route` is untouched. It is HTTP's dispatch unit, and the agnosticism sits one level above it.
- Dispatch stops travelling through the provider role channel: `ProviderRole::RpcController`, `ProviderRole::GrpcService`, `Module::dispatch_targets` and `dispatch_target_tokens` are all removed.
- Injecting a dispatch target fails as an ordinary missing dependency. The named refusal is removed with the token set that made it possible.

Scope is deliberately declaration and registration, not construction: `ControllerInstance`, `RpcControllerSource` and `GrpcServiceSource` each keep their own singleton-or-per-call fork, because each resolves its instance where its transport hands control back. Enhancer-token resolution likewise keeps its per-transport timing, noted in Consequences as a separate question.

Four roads not taken are recorded with their reasons: generalising `Route`, a third module list, renaming `controllers:`, and a registrar callback in place of the enum.

A stale field comment is corrected alongside: `dispatch_targets` still named RPC controllers alone after gRPC services joined it.
