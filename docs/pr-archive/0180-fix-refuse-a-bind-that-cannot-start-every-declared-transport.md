# #180 — fix!: refuse a bind that cannot start every declared transport

Merged 2026-08-23 into `master` from `fix/bind-refuses-a-half-bound-app`, commit [`131fbec`](https://github.com/ulo-rs/ulo/commit/131fbec33767b09dda4c78f3c4ad577ac7c815bc).

`ToniApplication::bind` wires four transports. Only the HTTP path propagated failure: WebSocket, RPC and gRPC registration and socket errors were logged at `error` level and execution continued, as were four configuration errors — a same-port gateway with no HTTP adapter, a separate-port gateway with no WebSocket adapter, RPC controllers with no RPC adapter, and a socket handed to `use_websocket_listener` for a port no gateway declares.

An application whose RPC adapter could not take its port therefore returned `Ok` and served HTTP with that transport absent. `BoundAdapters` cannot report the difference: `rpc: None` is equally the correct value for a subject-based transport that has no local listener. The failure was reachable only by reading logs, while the readiness probe answered 200 and the exit code stayed unset. Callers of the missing transport saw a connection refusal that looks like a network problem.

Each failure returns a `BindError`: `Adapter { transport, source }` when an adapter will not take its handlers or acquire its socket, `Setup` when a declaration has nothing registered to serve it. Sockets acquired before the failure are closed and dropped before the error returns. A failed bind is terminal — `bind()` consumes the adapters it wires, so a retry would report having none rather than the conflict that stopped it.

### Components

- **toni** — `BindError` gains an `Adapter` variant and `#[non_exhaustive]`; `bind()` splits into an outer state/teardown wrapper and the wiring itself; `AppState::Failed` makes a second bind say what is true.
- **docs** — ADR 0024 records the decision, including the lenient reading that was rejected and why reporting through `BoundAdapters` would not have worked.
- **tests** — five refusals covered, including a socket released after a later transport fails. One existing test declared RPC patterns behind an HTTP-only application and now registers the transport it declares.

### Breaking

`bind()` returns `Err` where it previously returned `Ok` and logged. An application that compiles in more transports than it wires fails at startup instead of serving the subset it managed; deployments that vary by transport select their module set or their adapters per binary. Matches on `BindError` need a wildcard arm.
