# #154 — An RPC controller can be built per call

Merged 2026-08-18 into `master` from `feat/rpc-builds-per-call`, commit [`f7a2d31`](https://github.com/ulo-rs/ulo/commit/f7a2d3166150f3ad6f75fb31b5d937d0dcb797c4).

An RPC call is an execution, and a controller serving one can now be built inside it. A controller whose dependencies belong to the call gets them; one that declares nothing is still built once at startup.

- `RpcControllerSource` carries what a controller declares — its token, patterns and enhancer tokens — and answers `instance` for one call. `RpcControllerTrait` keeps `handle_message`, the one thing that describes an instance. The framework read all four off an instance at startup, which is why a controller built per call could not register.
- `#[rpc_controller(scope = "request")]` hands the source the controller's own provider. Each call resolves one through it, so the call's execution cache is what serves it: a dependency a guard already resolved is the same instance the controller gets, and init/bootstrap fire on the instance the call is served by.
- A controller that declares no scope but depends on a request-scoped provider is elevated with a warning naming the dependency, as an HTTP controller is. That was a startup abort whose message told the user to change the dependency's scope.
- An RPC controller is no longer resolvable as a dependency. Its scope follows its dependencies, so a holder could not know whether it held one instance or one per call. The instance moves to a second module collection rather than being dropped — the provider map it leaves is also what the lifecycle hook loops read.

The instance is asked for where the handler runs, after guards, interceptors and pipes, and inside the same `catch_unwind` as the handler body: a rejected call builds no controller, and a panicking constructor renders an envelope.

Deferred to a follow-up: gRPC, which needs the same bridge routed through `#[grpc_methods]`'s generated tonic impl.
