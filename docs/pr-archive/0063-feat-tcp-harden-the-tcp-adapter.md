# #63 — feat(tcp): harden the TCP adapter

Merged 2026-05-01 into `master` from `feat/tcp-hardening`, commit [`01c816b`](https://github.com/ulo-rs/ulo/commit/01c816b176b7d96c378ca9175edce7e270f31bcf).

## Summary

Brings the toni-tcp adapter up to the same operational shape as toni-udp: failures surface at startup, shutdown is observable, and the client survives a server restart.

- `TcpAdapter` server: bind happens synchronously (using `std::net::TcpListener` + `from_std`) so port-in-use surfaces as `Err` from `app.start()` instead of panicking inside the spawned accept loop. Accept loop selects against a `watch::Sender<bool>`, and `RpcAdapter::close` drives the loop to exit so `ShutdownHandle::completed().await` resolves cleanly.
- `TcpClientTransport`: the `OnceCell` cache is replaced with a shared mutex slot. The reader loop clears it on exit; `send`/`emit` clear it on writer error. The next call rebuilds the TCP connection. Pending requests at the moment of failure still surface as `Transport("connection closed")`.

Per-connection tasks aren't awaited during shutdown — they finish when their clients disconnect or are cancelled with the runtime. Awaiting in-flight requests is deliberately out of scope.

## Test plan

- [x] `cargo build -p toni-tcp`
- [x] `cargo test -p toni-tcp --lib` — reconnect-path unit test passes
- [x] `cargo test -p integration-tests --test integration rpc_tcp` — both `rpc_handler_panic_returns_error_and_keeps_connection_alive` and `tcp_app_shutdown_stops_the_accept_loop` pass
