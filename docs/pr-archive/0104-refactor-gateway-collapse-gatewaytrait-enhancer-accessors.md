# #104 — refactor(gateway): collapse GatewayTrait enhancer accessors

Merged 2026-06-18 into `master` from `refactor/gateway-trait-deenrich`, commit [`2603a8e`](https://github.com/ulo-rs/ulo/commit/2603a8e3b861445de774e1c1a7d43d4491eeb020).

Folds `GatewayTrait`'s nine enhancer-token accessors — `get_guard_tokens` / `get_interceptor_tokens` / `get_pipe_tokens` / `get_error_handler_tokens`, the four per-handler `get_handler_*_tokens(event)`, and `get_handler_events` — into a single `enhancers()` method returning a `GatewayEnhancers` descriptor (gateway-level tokens + a per-event list). The resolver reads it once instead of calling a dozen methods.

Pure internal refactor: the gateway macro emits the descriptor, every existing gateway is untouched, behavior is unchanged (232/232 integration tests pass).

This is the substrate for the injectable-style gateway DX (`#[websocket_gateway]` on the struct + `#[subscriptions]` on the impl, per the controller pattern in #103). With the accessor surface collapsed, the struct attribute that emits `impl GatewayTrait` will bridge only the handful of behavior methods rather than a dozen.

Deferred to that follow-up: the `#[websocket_gateway]`/`#[subscriptions]` split itself; the same de-enrichment + DX for `#[rpc_controller]`.
