# #103 — feat(controller): declare like #[injectable], scan routes via #[routes]

Merged 2026-06-18 into `master` from `feat/controller-injectable-dx`, commit [`1e2695c`](https://github.com/ulo-rs/ulo/commit/1e2695c7d7f92ebd4441633adf4fcdfbcac3714c).

Brings controllers to parity with `#[injectable]`: `#[controller("/p")]` goes on the struct (`#[inject]` fields, `#[new]` constructor, lifecycle hooks), and a sibling `#[routes]` impl adds the handlers. The inline-struct form (`#[controller("/p", pub struct …)]`) is removed.

```rust
#[controller("/users")]
pub struct UsersController {
    #[inject] svc: UserService,
}

#[routes]
impl UsersController {
    #[get("/")] async fn list(&self) -> impl IntoResponse { /* … */ }
}
```

**Macros**
- `#[controller]` (struct attribute) produces a complete controller on its own — the `ControllerFactory`, the `Controller` object, and the inherent bridges (`__toni_build_from_deps` / `__toni_dependencies` / `__toni_prefix` / `__toni_is_request_scoped`). Field injection, `#[new]`, and lifecycle reuse the provider `__construct` / `__lifecycle` bridges. Same-token fields are deduplicated scope-aware. A controller with no `#[routes]` impl is valid and exposes zero routes.
- `#[routes]` (impl attribute) is purely additive: it scans `#[get]`/`#[post]`/… handlers into per-route `Route` wrappers and emits an inherent `__toni_routes` that shadows the empty `RoutesBridge` default. Construction, prefix, and scope are delegated to the struct bridges; the full path is `__toni_prefix()` joined with each handler's sub-path at registration time.
- Removed: the inline-struct form, bare-`new()` auto-detection, and `init = "…"`. Construction is `#[new]` or field injection, as with `#[injectable]`. A `#[routes]` impl with no `#[controller]` struct fails to compile at the bridge call sites.

**Core**
- `toni::__route::RoutesBridge` — the empty-default routes bridge that makes a bare controller valid.
- `toni::http_helpers::join_route` composes the prefix and a handler sub-path at registration time.

**Migration**
- All 131 controllers (examples, integration tests, and the actix/salvo/diesel/terminus crates) moved to the new form; real constructors tagged `#[new]`.

**Docs**
- ADR-0004 records the decision (including why an impl marker over `inventory`) and extends ADR-0002; `CLAUDE.md` updated.

Deferred: `#[websocket_gateway]` and `#[rpc_controller]` keep their current form; the same struct-attr + impl-marker treatment applies to them in a follow-up.
