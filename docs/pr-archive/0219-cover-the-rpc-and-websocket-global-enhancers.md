# #219 — Cover the RPC and WebSocket global enhancers

Merged 2026-08-31 into `master` from `test/global-enhancers-rpc-ws`, commit [`890662f`](https://github.com/ulo-rs/ulo/commit/890662f68cc9b6bc9f047dee1dfc94708f18b6f4).

`use_global_rpc_guards`, `use_global_ws_guards` and their interceptor and error-handler siblings appeared nowhere in the test suite. They work — but nothing would have said if they stopped.

Eight tests, four per transport, each keyed on a recorded call order: a global enhancer runs ahead of the controller's or gateway's own, a rejecting global guard stops the call there, a global interceptor wraps the chain below it, and a global error handler reshapes what the handler left.

## Two things the WebSocket assertions record

Neither is changed here. Both are pinned as they are, so the day either changes, a test says so.

**A rejected message answers the caller with nothing.** `GatewayWrapper` returns `WsError::AuthFailed`, and the message callback drops that variant deliberately to keep the connection usable. Keeping the connection is right; the silence is the other half. WebSocket is the only transport that answers a rejection with nothing — HTTP sends 403, RPC a `forbidden` frame, gRPC `PermissionDenied` — and a client awaiting a reply waits forever. The test helper takes a timeout for that reason and reports `<no reply>` rather than hanging.

**A connect runs guards but never enters the interceptor chain.** `begin_connect` resolves and runs guards and returns; only the message path builds the chain. ADR-0016 makes a connection an execution of its own, so an interceptor that times every execution misses every connection.
