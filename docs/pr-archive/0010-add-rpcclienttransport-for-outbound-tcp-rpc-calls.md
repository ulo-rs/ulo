# #10 — Add RpcClientTransport for outbound TCP RPC calls

Merged 2026-03-23 into `master` from `feat/rpc-tcp-client`, commit [`aeef808`](https://github.com/ulo-rs/ulo/commit/aeef8084453396140451cce47a66eb02de106385).

Introduce a new `RpcClientTransport` for making outbound TCP RPC calls, enabling request-response communication across process boundaries. Update the TCP adapter to allow binding to a configurable host instead of a hardcoded address, enhancing security and flexibility.
