# #91 — feat!: one #[injectable] provider form, marker-free enhancers, unified lifecycle

Merged 2026-06-07 into `master` from `spike/derive-injectable`, commit [`6dabbb5`](https://github.com/ulo-rs/ulo/commit/6dabbb5502d61ab710cefe8e7ac7d7f9a768d8d0).

## Summary

Reworks how providers, enhancers, and lifecycle hooks are declared so that each is plain Rust — a
struct plus its trait impls — with no parallel macro bookkeeping to keep in sync. A guard is a
provider that implements `Guard`; a constructor is a `fn` tagged `#[new]`; a lifecycle hook is a `fn`
tagged `#[on_module_init]`. Nothing has to be declared twice.

This is a breaking change to the provider/enhancer/lifecycle surface (details below); no runtime
behavior of HTTP/WS/RPC/gRPC dispatch changes.

## Providers

- `#[injectable]` is now an attribute on the struct: `#[inject]` fields are dependencies,
  `#[default(expr)]` fields are owned state, `#[injectable(scope = "request" | "transient")]` sets the
  scope. The macro supplies the `Clone` impl the container needs, so the struct carries no derive
  ceremony.
- The old struct-in-attribute form `#[injectable(pub struct Foo { .. })] impl Foo { .. }` is removed.
- `#[new]` marks a dependency-injected constructor (`fn new(dep: D, ..) -> Self`): parameters are
  resolved from the container — including dependencies used but not stored as fields, and
  request-scoped dependencies — replacing the old `init = "…"` / `from_request` conventions. A
  request-scoped provider reads request data by injecting the built-in `Request`.

## Enhancers

- A guard / interceptor / pipe / error-handler / middleware is an ordinary provider that implements
  the corresponding trait (`Guard<HttpContext>`, `Interceptor<WsContext>`, …). The role is detected
  from the impl at registration; the `#[guard]` / `#[interceptor]` / `#[pipe]` / `#[error_handler]` /
  `#[middleware]` marker attributes are removed.
- Detection covers all transports and scopes, and the `provider_factory!` / `provider_value!`
  registration macros (their explicit `guard` / `interceptor` arguments are removed).
- `#[use_guards(..)]` and the other application sites are unchanged. A type that doesn't implement the
  role it's referenced as now fails at the use site rather than silently going unregistered.

## Lifecycle

- One Nest-style name set across providers, controllers, and modules: `#[on_module_init]`,
  `#[on_application_bootstrap]`, `#[on_module_destroy]`, `#[before_application_shutdown]`,
  `#[on_application_shutdown]`.
- All five are uniformly `async fn(&self[, signal]) [-> InitResult]`. Module hooks were sync and took
  a container argument; they are now async and drop the container, matching provider/controller hooks.

## Migration

- `#[injectable(pub struct Foo { .. })] impl Foo { .. }` → `#[injectable] struct Foo { .. }` with the
  `impl` left as-is; a `new()` constructor gains `#[new]`.
- Drop `#[guard]` / `#[interceptor]` / `#[pipe]` / `#[error_handler]` / `#[middleware]` — the trait
  impl is the declaration.
- Module hooks become `async fn on_module_init(&self) -> toni::InitResult { ..; Ok(()) }` (no
  container parameter).

## Deferred

- Generic structs on `#[injectable]` (rejected with a clear error for now).
- ADRs for the load-bearing decisions (one provider form; the detect-from-impl bridge pattern;
  keeping the module/controller scan rather than the bridge) — to follow once this lands.

## Verification

229 integration + 59 unit tests pass; workspace and all examples build.
