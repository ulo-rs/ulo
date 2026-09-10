# #231 — Make panic handling uniform across the four transports

Merged 2026-09-02 into `master` from `fix/uniform-panic-policy`, commit [`b563659`](https://github.com/ulo-rs/ulo/commit/b56365982bc2791549cde324953daa02a69dd512).

Every panic a user's code can raise is now caught, delivered to the error chain if there is anything to deliver it to, and logged only where there is not. The two segments left out of that are closed here, and the logging that had drifted apart is settled on one rule.

| Segment | HTTP | RPC | WebSocket | gRPC |
| --- | --- | --- | --- | --- |
| `Middleware` | chain · `debug` | — | — | — |
| Guard | chain · `debug` | chain · `debug` | chain · `debug` | chain · `debug` |
| Guard, on WS connect | — | — | refuse · `error` | — |
| Interceptor | chain · silent | chain · silent | chain · silent | chain · silent |
| Handler body | chain · silent | chain · silent | chain · silent | chain · silent |
| Chain handler | `error` + position | `error` + position | `error` + position | `error` + position |
| Renderer | `error` + envelope | `error` + envelope | `error` + envelope | — |

The empty cells are structural: `Middleware` is an HTTP-only role, gRPC has no rendering step because `GrpcStatus` is its wire shape, and a WS connect guard refuses an upgrade that has no answer to shape — the one guard panic that reaches nobody, and so the one that reports.

- **gRPC** — an interceptor panic was built inside the link chain, where neither the enhancers nor the method name are in scope, and the wrapper turned the pipeline `Err` into a status without consulting the chain. The event now travels back to `run_grpc_pipeline` in a slot, which holds both, using the side-channel the generated wrapper already uses for a handler panic.
- **HTTP** — `Middleware::handle` ran outside panic recovery, so an unwind escaped the dispatcher into the adapter, which drops the connection. It is wrapped, and the caught event keeps its type through `MiddlewareResult`'s boxed error, so the chain gets a `PanicRecovered { Middleware }` rather than a stringified `MiddlewareFailure`.
- **Logging** — a guard panic reaches the chain everywhere, so the `error!` lines on RPC and gRPC were the framework deciding for the application what deserves an alert, when a catcher can log it at whatever level it chooses. They drop to `debug`. `error!` stays where the event can reach nobody.

Tests: a catcher reshapes the middleware panic and the gRPC interceptor panic, reading the segment back off the event. Reverting either fix fails its test — a dropped connection on HTTP, `Internal` instead of the catcher's status on gRPC.

Closes `FRAMEWORK_GAPS.md` F23 and F24.
