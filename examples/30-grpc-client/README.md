# 30 grpc-client

A generated client used as an ordinary provider, and a deadline that travels with the call.
`Front` is a gRPC service that calls `Clock`; it injects the `ClockClient` that
`GrpcClientModule` binds, and forwards its own deadline with `ulo_grpc::outgoing`.

```
cargo run -p ex-30-grpc-client
```

The server listens on `PORT` (default 50052) and prints `listening on http://ADDR`. Without
`CALLEE` the binary also serves `Clock` on a free port and the front calls it; with `CALLEE` set,
the front calls that address instead. The channel connects on the first call, so the front starts
whether or not the callee is up.

Read first:

- `src/lib.rs`: `FrontModule` imports `GrpcClientModule::<ClockClient<Channel>>::for_root(..)`;
  `Front` takes `Dep<ClockClient<Channel>>`. `outgoing(exec, request)` sets `grpc-timeout` from the
  execution's remaining time, and refuses a deadline that has passed with DEADLINE_EXCEEDED before
  any request exists.
- `tests/client.rs`: the callee reports the time left of the caller's deadline; a spent deadline
  is refused and the callee's call count stays 0 (the execution is opened with a deadline already
  in the past, so no test waits on a clock); a callee that is down fails the first call.
- `tests/binary.rs`: the binary against an in-process callee, then SIGTERM.

Next: [31 graphql](../31-graphql).
