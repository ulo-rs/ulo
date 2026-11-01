# 41 one guard, two transports

One `impl<T: Transport> Guard<T>` guards an HTTP route and an RPC message. It reads the `#[meta(Shut)]` a handler declares through `AsRef<ExecutionRef>`, which every transport's context implements, and each transport renders the refusal its own way: a 403 `application/problem+json` over HTTP, a `Forbidden` error over RPC.

```
cargo test -p ex-41-one-guard-two-transports
cargo run -p ex-41-one-guard-two-transports
curl -i http://127.0.0.1:3000/shop/stocktake
```

The binary listens on `PORT` (HTTP, default 3000) and `RPC_PORT` (TCP, default 7000) and prints a `listening on` line for each; the RPC call goes through `RpcClient` as in example 25.

Read first:

- `src/lib.rs`: `Maintenance`, the guard generic over the transport, and the two controllers that both name it in `#[guards]`.
- `tests/guard.rs`: the open handler is admitted on both transports; the shut one is refused on both, with every field each transport carries.

Previous: [40 socket-activation](../40-socket-activation). Examples 16 (HTTP enhancers) and 25 (RPC basics) introduce each side.
