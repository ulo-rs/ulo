# 40 socket activation

An application serving on a socket it did not open. A supervisor (systemd, `ulo dev --listen`, a
test) binds and listens, then starts the process with the socket as file descriptor 3 and
`LISTEN_FDS=1`, `LISTEN_PID=<the process>`. The app names the socket with
`Server::new(Endpoint::inherited_index(0))` and `listen` takes it; ulo reads the variables itself,
so no `listenfd` crate is needed. The point: connections made before the app exists wait in the
kernel's backlog and are answered once it is up, so a restart drops none.

```
PORT=3000 cargo run -p ex-40-socket-activation              # no LISTEN_FDS: binds 127.0.0.1:PORT as 01 does
ulo dev --listen 127.0.0.1:3000 -p ex-40-socket-activation  # ulo dev holds the socket across rebuilds
```

With `LISTEN_FDS` set, `main` adopts descriptor 3 instead. `ulo dev --listen` is one supervisor;
systemd's `Sockets=` is another, and `tests/binary.rs` plays one (a parent socket crossing a shell
`exec`). Under `ulo dev --listen ADDR` an application that never names an inherited socket adopts
the held one too: `ULO_DEV=1` makes a plain `Server::new("ADDR")` resolve to it (see
`EndpointSpec::resolve`), so example 01 restarts under `ulo dev --listen 127.0.0.1:3000 -p
ex-01-hello` without refusing a connection.

Unix only: `Activation` answers `Unsupported` elsewhere, and both test files are `#[cfg(unix)]`.

Read first:

- `src/lib.rs`: `server`, the one place that chooses between an address and an inherited socket.
- `tests/binary.rs`: the early connection served after the app starts; a UDP socket at descriptor 3
  (`NotListening`) and a `LISTEN_PID` of another process (`PidMismatch`), both failing startup.
- `tests/refusal.rs`: an inherited index or name nobody handed over is `StartupError::Configure`
  naming it.

Not shown: handing in an already-bound `std::net::TcpListener` without the environment protocol.
The public API has no such path yet; `Endpoint::listener` is the agreed design (ledger F345).

Previous: [39 one-app](../39-one-app). Next: [41 one-guard-two-transports](../41-one-guard-two-transports).

