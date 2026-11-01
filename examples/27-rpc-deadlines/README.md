# 27 rpc deadlines

A gateway calls a pricing service through a keyed `RpcClientModule` (`Dep<RpcClient, Pricing>`). A call made `.within(&exec)` sends what is left of the caller's deadline as `deadline-ms`, so the callee's execution ends when the caller gives up. A call made once the deadline has passed fails with `Timeout` before it connects: the callee's handler does not run.

The callee is given the shorter of the caller's time and the client's timeout (five seconds unless set), so a caller with a longer deadline than that reaches the callee with five seconds.

```
cargo test -p ex-27-rpc-deadlines
PORT=7000 cargo run -p ex-27-rpc-deadlines
```

The binary runs both services in one process: the pricing service on a port of its own, the gateway on `PORT`.

Read first:

- `src/lib.rs`: the callee's handler, which reports its time left and counts its runs; the gateway and its keyed client.
- `tests/deadlines.rs`: the callee's time left is at most the gateway's, which is at most the caller's; an execution with a spent deadline answers `Timeout` and the run counter stays at zero.

Next: [28 rpc links](../28-rpc-links).
