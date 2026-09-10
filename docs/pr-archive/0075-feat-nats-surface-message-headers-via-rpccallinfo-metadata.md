# #75 — feat(nats): surface message headers via RpcCallInfo.metadata

Merged 2026-05-06 into `master` from `feat/nats-rpc-metadata`, commit [`66f09e4`](https://github.com/ulo-rs/ulo/commit/66f09e46c95d5a2085fba75629ffabb5f683f5b4).

NATS messages carry a HeaderMap natively; the adapter now copies each
header's first value into `RpcCallInfo.metadata` at message receipt, so
RPC handlers reading `ctx.metadata()` see what the wire actually sent
instead of an empty map.
