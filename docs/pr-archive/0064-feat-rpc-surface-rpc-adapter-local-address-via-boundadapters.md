# #64 — feat(rpc): surface RPC adapter local address via BoundAdapters

Merged 2026-05-02 into `master` from `feat/rpc-bound-address`, commit [`8432865`](https://github.com/ulo-rs/ulo/commit/84328658c9925a831e69c110e46f4752f38b2fff).

Listener-based RPC adapters (TCP, UDP) already bind synchronously inside `RpcAdapter::bind` after the recent hardening PRs, but the resulting `SocketAddr` never made it back to the caller. Two consequences:

- Anyone passing port 0 (test code, ephemeral binding) has no way to learn the OS-assigned port — RPC integration tests retry-connect to compensate.
- Future RPC transports (gRPC, MQTT, RabbitMQ, etc.) would inherit the same gap by cloning the current adapter shape.

This PR closes that gap framework-wide so new adapters surface their address by default.

## What's in the box

- `RpcAdapter` trait — adds `fn local_addr(&self) -> Option<SocketAddr>` with a default `None` impl. Subject-based transports (NATS, future Kafka) opt out for free.
- `BoundAdapters` — adds `rpc: Option<SocketAddr>`, populated from `local_addr()` after a successful `bind()`.
- `TcpAdapter` and `UdpAdapter` — capture the listener/socket address at bind time and return it from `local_addr()`.
