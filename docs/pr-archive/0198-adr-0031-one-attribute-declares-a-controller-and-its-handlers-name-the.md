# #198 — ADR 0031 — one attribute declares a controller, and its handlers name the transport

Merged 2026-08-28 into `master` from `docs/adr-one-controller-attribute`, commit [`cc5c0aa`](https://github.com/ulo-rs/ulo/commit/cc5c0aa956c9bb0c0102d954d8336f4e73abe854).

Records the proposal that `#[controller]` becomes the only struct attribute for dispatch targets, with the handler impl naming the transport: `#[routes]` answers HTTP, `#[patterns]` RPC, `#[grpc_methods]` gRPC.

After ADR-0030 the three struct attributes differ only in name, in the `Dispatch` variant their generated `dispatch()` hands over, and in which attribute their elevation warning names — and the handler impl already carries the transport. `#[grpc_service]` is also the last holder of the inline-struct form: with a shared struct attribute, the declaration gains a struct site and the inline form dies.

The proposal:

- `#[rpc_controller]` and `#[grpc_service]` are removed outright — no migration errors, the crate has no users to migrate.
- A `DispatchBridge` replaces `RoutesBridge`: blanket default answering `Dispatch::Http(Vec::new())` (a controller with no handler impl stays valid), shadowed by each handler-impl macro. The RPC source companion moves from the struct attribute into `#[patterns]`.
- One transport per struct, enforced by the duplicate inherent `__toni_dispatch` failing to compile.
- A route prefix on a non-HTTP controller warns at startup.
- A gRPC service becomes a plain struct under `#[controller]`, a plain inherent impl, and `#[grpc_methods]` on the tonic trait impl.

Three roads not taken are recorded: transport markers on the struct attribute, hybrid multi-transport controllers, and alias names.

Depends on #196.
