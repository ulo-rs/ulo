# 25 rpc basics

A controller answers calls by pattern over TCP. `#[ulo_rpc::message]` answers its caller, with the handler's value or its error and that error's kind; `#[ulo_rpc::event]` answers nobody. A pattern no handler claims is refused with the reason `pattern_unhandled`.

An event reaches one server instance, as a request does. `#[ulo_rpc::event("pattern", every_instance)]` reaches each instance listening on the pattern over a broker (example [28](../28-rpc-links)); TCP's caller names one server, so the server refuses that option at startup here.

```
cargo test -p ex-25-rpc-basics
PORT=7000 cargo run -p ex-25-rpc-basics
```

There is no HTTP request to make: the binary prints `listening on tcp://ADDR`, and a call goes through `RpcClient` (`request`, `emit`).

Read first:

- `src/lib.rs`: the calculator, its `Classify` error and the journal an event writes to.
- `src/roles.rs`: a `Guard<Rpc>` reading the `#[meta(RequiresRole(..))]` a handler declares, as example 16's guard does on HTTP; `calc.clear` declares `admin`, `calc.divide` declares nothing and stays open.
- `tests/rpc.rs`: a reply, a handler error, an event, an unhandled pattern, and the guard (no role and a wrong role are `Forbidden` and never reach the handler; `x-role: admin` is admitted).
- `tests/binary.rs`: runs the compiled binary with `PORT` set to a free port, calls the address it prints and ends it with SIGTERM.

Next: [26 rpc streams](../26-rpc-streams). The same guard generic over HTTP and RPC is [41](../41-one-guard-two-transports).
