# #43 — feat: method-level enhancers for WS gateways and RPC controllers

Merged 2026-04-18 into `master` from `feat/method-level-enhancers-ws-rpc`, commit [`21c0851`](https://github.com/ulo-rs/ulo/commit/21c0851f87de191d0655b66efea5c62f57d3a1ab).

## Summary

- `#[use_guards]`, `#[use_interceptors]`, `#[use_pipes]`, and `#[use_error_handlers]` on individual `#[subscribe_message]` / `#[message_pattern]` methods were silently ignored; this wires them up for both protocols
- RPC controllers had no enhancer support at all — controller-level and method-level are now both implemented
- Both follow the same global → controller → method stacking order as HTTP
- Fix: a guard blocking a WS message now keeps the connection alive (previously closed it), which is necessary for per-handler isolation

## Test plan

- [ ] `cargo test -p integration-tests --test integration -- --no-capture` — 128 tests pass
- [ ] `method_enhancers::ws_method_level_enhancers_work` — all four enhancer types on a WS gateway, with isolation
- [ ] `method_enhancers::rpc_method_level_enhancers_work` — all four enhancer types on an RPC controller via TCP
