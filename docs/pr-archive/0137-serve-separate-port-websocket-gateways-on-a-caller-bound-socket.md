# #137 — Serve separate-port WebSocket gateways on a caller-bound socket

Merged 2026-08-12 into `master` from `feat/ws-listener-adoption`, commit [`53a873a`](https://github.com/ulo-rs/ulo/commit/53a873acab3e2abf37943a797a58cdd760e2af08).

A separate-port gateway can now be handed a socket the caller already bound, through `app.use_websocket_listener(declared_port, listener)`. `BindTarget` reached HTTP in #134 and RPC/gRPC take a listener through their constructors; a gateway's port lives in its attribute, so this is the transport that needed a mapping rather than a parameter.

The declared port is that mapping. It is the number written in `#[websocket_gateway("/p", port = N)]`, the key the adapter files its callbacks under, and now the key the caller names when supplying a socket. The socket may listen anywhere — `BoundAdapters::websocket` reports the address it listens on.

- **core** — `use_websocket_listener`; `WebSocketAdapter::into_lifecycle_handles` takes `Vec<(u16, BindTarget)>` in place of `Vec<(u16, String)>`, the hostname folding into the target's address arm. A socket matching no gateway is logged as an error at `bind()` and left unserved.
- **adapters** — axum, poem, salvo, and tungstenite resolve the target the same way they already do on their HTTP path. Actix and rocket implement no `WebSocketAdapter` and are untouched.
- **ADR 0014** — why the pairing is stated rather than inferred, and why this one goes through the SPI when RPC and gRPC go through a constructor.
- **tests** — per adapter, a gateway declaring a port nothing binds, served on a port-0 socket handed over by the test.

Breaking for out-of-tree `WebSocketAdapter` implementations. Every in-tree one is updated.
