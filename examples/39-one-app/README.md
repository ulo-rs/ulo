# 39 one-app

HTTP, WebSocket, RPC and gRPC in one app: one configuration, one store, one set of health probes, one `close`. Each transport has a handler that writes a note to the store and a `hold` that waits on a gate, so a test can keep one execution in flight on every transport while the app closes.

```
cargo test -p ex-39-one-app
PORT=3000 APP_PORTS__WS=3001 APP_PORTS__RPC=7000 APP_PORTS__GRPC=50051 cargo run -p ex-39-one-app
curl -X POST http://127.0.0.1:3000/notes/anvil
curl http://127.0.0.1:3000/health/ready
```

`main` prints one `listening on SCHEME://ADDR` line per server in the order it bound them (`http`, `ws`, `tcp` for RPC, `grpc`), then `store open`; `/health/ready` is 503 naming `store` until that line. On SIGTERM it prints `closed on SIGTERM`.

The combined concepts, each in its own example: [15 http-requests](../15-http-requests) and [18 http-streaming](../18-http-streaming), [23 ws-gateway](../23-ws-gateway), [25 rpc-basics](../25-rpc-basics), [29 grpc-service](../29-grpc-service), [33 config](../33-config), [34 health](../34-health), [10 shutdown](../10-shutdown). The store is a plain in-memory provider rather than the databases of 35-37, so the example needs no service. `ulo new` writes a project with configuration and health wired the same way (`crates/ulo-cli/src/templates/new`).

Read first:

- `src/lib.rs`: the wiring. The controllers are registered in `AppModule` because they read the configuration it imports.
- `src/store.rs`: the store, its health check and the gate. `src/http.rs`, `src/ws.rs`, `src/rpc.rs`, `src/grpc.rs`: one handler pair each.
- `tests/answers.rs`: every transport answers and writes to the same store; the servers report their addresses in bind order.
- `tests/drain.rs`: one `close` on a scripted clock. `/ready` is 503 from its start; after the lag, all four listeners refuse; an execution released inside the window finishes on each transport and the WebSocket closes with 1001; one held past the window is refused with the shutdown phrase on each and counted in `Shutdown::abandoned`.
- `tests/health.rs`: 503 naming the closed store until it opens.
- `tests/binary.rs`: runs the compiled binary with a port per transport and ends it with SIGTERM.

`Shutdown` carries the signal and two counts and names no transport, so the order in which the transports close is not observable; the tests assert the order they bind in.

Next: a project of your own. `ulo new <name>` writes one with this wiring for the transport it is given (`crates/ulo-cli`).
