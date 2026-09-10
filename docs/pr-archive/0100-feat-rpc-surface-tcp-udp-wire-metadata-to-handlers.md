# #100 — feat(rpc): surface TCP/UDP wire metadata to handlers

Merged 2026-06-12 into `master` from `feat/tcp-udp-metadata`, commit [`a06499c`](https://github.com/ulo-rs/ulo/commit/a06499c0d69f3b8530998b67f6b8a2d544472bc1).

The TCP and UDP adapters built a bare `RpcCallInfo`, dropping any metadata the caller put on the frame, so `RpcContext::metadata()` was always empty for those transports — while the brokers and NATS already surfaced it. Both adapters now parse the frame's `metadata` object into `RpcCallInfo`, completing the per-call metadata path across every transport and closing the long-standing TCP/UDP metadata gap.

## Changes

- **Server** ([tcp_adapter.rs](toni-tcp/src/tcp_adapter.rs), [udp_adapter.rs](toni-udp/src/udp_adapter.rs)) — parse the inbound frame's `metadata` object (string values) into `RpcCallInfo.metadata`.
- **Tests** ([rpc_tcp.rs](integration-tests/tests/integration/rpc_tcp.rs), [rpc_udp.rs](integration-tests/tests/integration/rpc_udp.rs)) — a handler echoes `ctx.get_metadata("trace")`; the client sets it via `RpcClient::request().metadata(..)` and asserts the round-trip. Verified the tests fail with extraction disabled.

The client side (the `metadata` envelope field) already shipped, so this is purely the server-read half.
