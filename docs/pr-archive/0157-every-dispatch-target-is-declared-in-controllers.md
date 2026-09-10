# #157 — Every dispatch target is declared in controllers:

Merged 2026-08-19 into `master` from `feat/controllers-hold-dispatch-targets`, commit [`374bccf`](https://github.com/ulo-rs/ulo/commit/374bccf4284d392d0f957118a13cf767dbdc6c4a).

RPC controllers and gRPC services are declared in their module's `controllers:` list, beside HTTP controllers, and `providers:` holds only what something may hold. Implements ADR 0017 (#156).

A controller hands over a `Dispatch` rather than a route list:

```rust
pub enum Dispatch {
    Http(Vec<Arc<dyn Route>>),
    Rpc(Arc<dyn RpcControllerSource>),
    Grpc(Arc<dyn GrpcServiceSource>),
}
```

`Route` is unchanged — it is HTTP's dispatch unit, and generalising it would push HTTP's shape onto transports without one.

**Macros.** `#[rpc_controller]` and `#[grpc_service]` emit a `ControllerFactory` and a `Controller` object instead of a provider factory. The request-scope elevation scan comes with them; the controller factory already ran that check for HTTP. Only the per-call shape still needs a `Provider`, to resolve its dependencies inside the call, and lifecycle hooks fire from the controller object where an HTTP controller's already fire.

**Deletions.** `ProviderRole::RpcController`, `ProviderRole::GrpcService`, `Module::dispatch_targets` and the container's dispatch-target token set all go. Registration reaches the role registry from the controller path, and a dispatch target is absent from the provider store structurally rather than by exception.

**Lifecycle fix.** `ApplicationContext` iterated providers alone for destroy, before-shutdown and shutdown, with `ToniApplication` adding the controller pass afterwards — so a context built without an HTTP server never ran a controller's shutdown hooks. The pass moves into the context, and the three wrappers that added it collapse to delegation. Every dispatch target is a controller after this change, and the gap would otherwise have widened from HTTP controllers to every transport.

**Hand-written controllers.** `toni-async-graphql` and `toni-juniper` implement `Controller` directly and are updated; the trait change reaches beyond the macros.

## Behaviour change

Injecting a dispatch target no longer produces a named refusal. Declaring one in `providers:` does not compile, there being no provider factory to name; reaching for one through `#[inject]` fails as an ordinary missing dependency. ADR 0017 records the weaker message as the accepted cost of the token set going away.

## Migration

Move `#[rpc_controller]` and `#[grpc_service]` structs from `providers:` to `controllers:`. Every declaration in the repo, its examples and its READMEs is updated.

## Tests

- both refusal fixtures assert the not-found failure and name the target
- a controller's teardown hooks run when an application context closes, with no HTTP server in play
- the same hooks run exactly once when a full application closes, which a second pass above the context would break

The two teardown tests each fail when falsified: suppressing the pass empties the log, and restoring the application's own pass duplicates an entry.

367 integration tests pass.
