# #56 — refactor(adapter): rename create → listen/serve; expose bound address

Merged 2026-04-25 into `master` from `refactor/adapter-listen-serve`, commit [`594c549`](https://github.com/ulo-rs/ulo/commit/594c54929844216d87921a243c67a2e626d0ba1e).

## Summary

- `HttpAdapter::create` and `WebSocketAdapter::create` are renamed to `listen`,
  which returns `Pin<Box<dyn Future<Output = Result<ServerHandle>>>>`. The future
  resolves once the socket is bound; `ServerHandle { local_addr, serve }` carries
  the actual bound address and the long-running accept loop separately.
- `RpcAdapter::create` is renamed to `serve` — broker-based transports (NATS, TCP
  message bus) don't own a port, so `listen` would have been misleading.
- `TcpListener::bind()` was previously buried inside the returned future, making
  the bound port invisible. Port 0 silently dropped the OS-assigned address, bind
  errors surfaced inside a running future, and there was no way to sequence startup
  around a known address. All of that is fixed here.
- All adapter implementations updated: `toni-axum`, `toni-actix`, `toni-tungstenite`,
  `toni-tcp`, `toni-nats`.

## Test plan

- [ ] `cargo build` passes with no new errors or warnings
- [ ] Start a server with port `0` and confirm `local_addr` in the log shows the
  OS-assigned port rather than `0`
- [ ] Existing integration tests pass unchanged — no test should need a port
  assignment strategy to avoid collisions now that port 0 is usable
