# 25 rpc basics

A controller answers calls by pattern over TCP. `#[ulo_rpc::message]` answers its caller, with the handler's value or its error and that error's kind; `#[ulo_rpc::event]` answers nobody. A pattern no handler claims is refused with the reason `pattern_unhandled`.

```
cargo test -p ex-25-rpc-basics
PORT=7000 cargo run -p ex-25-rpc-basics
```

There is no HTTP request to make: the binary prints `listening on tcp://ADDR`, and a call goes through `RpcClient` (`request`, `emit`).

Read first:

- `src/lib.rs`: the calculator, its `Classify` error and the journal an event writes to.
- `tests/rpc.rs`: a reply, a handler error, an event, an unhandled pattern.
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, calls the address it prints and ends it with SIGTERM.

Next: [26 rpc streams](../26-rpc-streams).
