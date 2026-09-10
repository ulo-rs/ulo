# #102 — refactor(core): collapse Controller to one object per struct

Merged 2026-06-15 into `master` from `refactor/controller-collapse-routes`, commit [`f2fffb0`](https://github.com/ulo-rs/ulo/commit/f2fffb035e0107f6646c71a949084ee5581cf3a0).

Models an HTTP controller as a single `Controller` per struct that exposes its routes, replacing the previous one-`Controller`-trait-object-per-route model. Lifecycle hooks now fire once per controller by construction, and the shutdown path — which previously fired controller hooks once per route — is corrected. No public `#[controller]` syntax changes.

**Core**
- `Route` trait carries per-route dispatch (`execute` / `get_path` / `get_method` / `enhancers` / `get_route_metadata` / `get_body_dto`); `Controller` keeps the token, `routes()`, and the lifecycle hooks; `ControllerFactory::build` returns one `Arc<dyn Controller>`.
- `ControllerInstance` holds the built instance (singleton path) or the resolved dependencies (request path); `routes()` matches on it to produce the route set.
- Drops the `get_controller_type_name` lifecycle dedup — the scanner and shutdown iterate one controller instance each.
- `InstanceWrapper` dispatches a `Route`; the module keeps controller instances for lifecycle alongside the per-route dispatch units.

**Macro**
- `#[controller]` emits one `…ControllerObject` exposing `routes()` plus a per-route `Route` wrapper, instead of N `Controller` wrappers.

**Adapters**
- `toni-async-graphql` and `toni-juniper` — the only hand-written controller impls — move their query and playground endpoints onto `Route` under one `Controller`.

Deferred to a follow-up: the per-route dispatch units could be derived from the controller objects at bind time rather than staged in a map, collapsing the dual storage to a single source of truth.
