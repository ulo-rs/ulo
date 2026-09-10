# #155 — A gRPC service can be built per call

Merged 2026-08-19 into `master` from `feat/grpc-builds-per-call`, commit [`1479f77`](https://github.com/ulo-rs/ulo/commit/1479f77a484c5ea41238e9c81b39947595b5941b).

A gRPC service is built inside the call it serves when its dependencies belong to that call, and stops being injectable.

`#[grpc_service(scope = "request", …)]` declares per-call construction. A service that declares no scope is elevated to it when any dependency turns out to be request-scoped, with a warning naming the dependency; the check is the one `#[controller]` and `#[rpc_controller]` already use. The per-call shape resolves inside the call's execution, so the service shares the instances the call already holds rather than starting its own.

`GrpcServiceTrait` becomes `GrpcServiceSource`, implemented on a companion generated beside the struct rather than on the struct itself. The token and the enhancer tokens are read at bind time, before any call exists, so reaching them through an instance is what a per-call service cannot do. Instances come from the companion instead, through an inherent `instance` rather than a trait method — the wrapper delegates via UFCS at the concrete type, and a trait object cannot answer that. The wrapper asks for one inside the delegate closure, where a rejecting guard has already had its say, and inside the same panic recovery as the handler body.

Injecting a service is refused with the diagnostic RPC controllers already get, which now names which kind of dispatch target it found. The instance stays in the collection kept out of dependency resolution, so the startup and shutdown hook loops still reach it.

Lifecycle hooks move onto the bridge for both dispatch targets. The per-call path has no `Provider` of its own to hang them on and reaches them by name instead, so the hooks scanned off the impl are re-emitted as the inherent forwarders the bridge dispatches to. Without that, elevating a service with a hook would have compiled and then fired nothing.

`GrpcAdapter::register_services` takes `Arc<dyn GrpcServiceSource>` in place of `Arc<Box<dyn GrpcServiceTrait>>`. `toni-grpc` is updated; no other adapter is affected.

## Tests

- a per-call service is built once per call, and its request-scoped dependency is the instance a guard already resolved
- a service declaring no scope is still built once and shared
- injecting a service is refused, asserted through a fixture binary because init failure exits the process
- a service's startup hooks still fire, covering the bridge rewiring that nothing else reaches

The first and the last each fail when the mechanism they cover is disabled.

## Deferred

WebSocket, because a connection is a session rather than an execution and the two scopes are a separate design question.
