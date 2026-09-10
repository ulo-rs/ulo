# #107 — refactor(rpc): collapse RpcControllerTrait enhancer accessors into one descriptor

Merged 2026-06-19 into `master` from `refactor/rpc-trait-deenrich`, commit [`353c3e0`](https://github.com/ulo-rs/ulo/commit/353c3e0a5d49c234ea930c8ac4d0fa2863f094d3).

Collapses `RpcControllerTrait`'s enhancer surface — four controller-level token accessors, `get_handler_patterns`, and four per-pattern getters keyed by string — into a single `enhancers() -> RpcEnhancers` descriptor (controller-level tokens plus a `handlers: Vec<RpcHandlerEnhancers>` list).

- **Trait:** `RpcEnhancers` / `RpcHandlerEnhancers` replace the nine accessors; `enhancers()` defaults to empty.
- **Resolver:** reads `enhancers()` once and iterates `handlers`, instead of calling four getters per pattern.
- **Macro:** `#[rpc_controller]` builds the descriptor in a single pass.

No behavior change — the RPC enhancer and dispatch tests pass unchanged.
