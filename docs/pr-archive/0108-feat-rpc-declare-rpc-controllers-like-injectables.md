# #108 — feat(rpc): declare RPC controllers like injectables

Merged 2026-06-20 into `master` from `feat/rpc-controller-injectable-dx`, commit [`ff5df29`](https://github.com/ulo-rs/ulo/commit/ff5df2974c82ef88533355db16af5e41c0cd3445).

Brings the injectable-style declaration form to RPC controllers — the last role to adopt it: `#[rpc_controller]` on the struct, `#[patterns]` on the impl, matching `#[injectable]`, `#[controller]`, and `#[websocket_gateway]`.

`#[rpc_controller]` sits on the struct with `#[inject]` fields and `#[new]`/lifecycle reached through the provider bridges; it emits the provider wiring (carrying the rpc-controller role) plus `impl RpcControllerTrait` with `get_token` baked, delegating `get_patterns` / `handle_message` / `enhancers` to a new `RpcHandlersBridge`. `#[patterns]` on the impl aggregates the `#[message_pattern]` / `#[event_pattern]` handlers into the pattern list and the `handle_message` match, plus the enhancer attrs. RPC has no connection hooks, so `#[patterns]` is pure aggregation — the only impl marker — and the pattern list, being impl-derived, bridges through it (unlike the gateway's attribute-baked path). A controller with no `#[patterns]` impl registers but routes nothing.

- **Library:** `toni::__rpc::RpcHandlersBridge`; the rpc-controller role preset on `generate_provider_from_struct`.
- **Macros:** `#[rpc_controller]` becomes a struct attribute; new `#[patterns]` impl attribute; the inline-struct form is removed.
- **Migration:** every RPC controller across examples and tests moved to the new form.
- **Tests:** a bare-controller test for the self-sufficiency guarantee; the RPC TCP/UDP and enhancer suites pass unchanged.
- **Docs:** ADR-0006.
