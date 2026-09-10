# #136 — Serve RPC and gRPC on a caller-bound socket

Merged 2026-08-12 into `master` from `feat/rpc-grpc-listener-adoption`, commit [`c3aa47c`](https://github.com/ulo-rs/ulo/commit/c3aa47cff46a28524cc51e9a741720ae1a528823).

Adds a second constructor to each transport adapter that owns a listening socket, taking one the caller already bound instead of an address to bind: `TcpAdapter::from_listener`, `UdpAdapter::from_socket`, `GrpcAdapter::from_listener`. `BindTarget` covers the HTTP side already; these three own their addressing in their constructors, so the listener reaches them there and the SPI is untouched.

- **toni-tcp** — stores a `BindTarget` and resolves it where it used to bind.
- **toni-udp** — a private datagram target, since `BindTarget` is TCP-typed and cannot carry a `UdpSocket`.
- **toni-grpc** — a private target holding a `SocketAddr`; routing one through `BindTarget`'s hostname string would re-resolve the address and drop an IPv6 scope id.
- **tests** — address identity per adapter, plus a real round-trip on the adopted socket.

The five broker transports (NATS, Redis, RabbitMQ, MQTT, Kafka) connect out to a broker and have no listening socket, so they are absent by construction.

Separate-port WebSocket is deferred to a follow-up: its addressing is owned by the gateway declaration rather than by a constructor, so it needs a mapping from declared port to socket that this shape does not provide.
