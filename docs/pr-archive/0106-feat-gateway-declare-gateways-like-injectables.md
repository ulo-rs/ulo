# #106 — feat(gateway): declare gateways like injectables

Merged 2026-06-19 into `master` from `feat/gateway-injectable-dx`, commit [`2ad4205`](https://github.com/ulo-rs/ulo/commit/2ad4205332ee4b2d7cc0d4a3c2d547b69e91b5dd).

Brings the injectable-style declaration form to WebSocket gateways: `#[websocket_gateway]` on the struct, `#[subscriptions]` on the impl — matching `#[injectable]` and `#[controller]`.

`#[websocket_gateway("/path", namespace = …, port = …)]` sits on the struct with `#[inject]` fields and `#[new]`/lifecycle reached through the provider bridges; it emits the provider wiring (carrying the gateway role) plus `impl GatewayTrait` with path/namespace/port baked in, delegating each behavior method to a `WsHandlersBridge`. On the impl side, `#[subscriptions]` aggregates the `#[subscribe_message]` handlers (and enhancer attrs) into `handle_event`, while the single-slot connection hooks `#[on_connect]` / `#[on_disconnect]` / `#[after_init]` are their own per-method macros (like `#[new]`/`#[on_module_init]`) — so a hook composes on its own, declaring one twice is a compile error rather than a silent last-wins, and a gateway with neither is a complete connection-only gateway via the bridge defaults.

- **Library:** `toni::__ws::WsHandlersBridge`; the gateway role preset on `generate_provider_from_struct`.
- **Macros:** `#[websocket_gateway]` becomes a struct attribute; `#[subscriptions]` plus standalone `#[on_connect]`/`#[on_disconnect]`/`#[after_init]`; the inline-struct form is removed.
- **Migration:** every gateway across examples and tests moved to the new form.
- **Tests:** end-to-end coverage for the two self-sufficiency cases — a bare gateway with no `#[subscriptions]`, and a connection-only gateway whose `#[on_connect]` fires with no `#[subscriptions]`. The existing WebSocket suite and adapter e2e tests pass unchanged.
- **Docs:** ADR-0005.

Also folds in a fix for adapter-test controllers in `toni-axum`, `toni-poem`, and `toni-rocket` left on the removed inline `#[controller]` form by #103 — their test targets compile only under `cargo test`, not plain `cargo build`, so the break was invisible.
