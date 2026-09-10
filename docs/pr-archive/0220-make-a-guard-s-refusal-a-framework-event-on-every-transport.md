# #220 — Make a guard's refusal a framework event on every transport

Merged 2026-08-31 into `master` from `fix/guard-rejection-is-an-event`, commit [`ca2289f`](https://github.com/ulo-rs/ulo/commit/ca2289f5cbea98d0e54a67e26d59dfaf9625898c).

HTTP builds a `GuardRejection`, fans it to observers and offers it to the error chain, so `#[catch(GuardRejection)]` can reshape a refusal. The other three transports built a transport error and returned it — no typed event, no chain — so a catcher never matched, while the reference documents that reshaping without qualification.

WebSocket had a second problem on top. `GatewayWrapper` returned `WsError::AuthFailed`, and the message callback dropped exactly that variant to keep the connection usable. Keeping the connection is right; answering nothing is not, and no other transport does it. A client that awaits a reply waits forever.

## After

| transport | typed event | offered to the chain | unclaimed rendering |
| --- | --- | --- | --- |
| HTTP | unchanged | unchanged | 403 |
| RPC | new | new | `forbidden` frame, as before |
| gRPC | already built | new | `PermissionDenied`, as before |
| WebSocket | new | new | canonical `{"status":"error","kind":"Forbidden",…}`, where there was silence |

Only WebSocket's output changes without a catcher present. RPC and gRPC render what they always did unless a handler claims.

## A refused connect is unchanged

There is no open connection to answer on, so declining the upgrade is the answer. That is the same reason a connect runs guards but not interceptors: a guard answers admission, which is its whole contract, while an interceptor wraps a call and its answer and an error handler shapes one — a connection has neither. `begin_connect`'s doc comment now says so, since it is the question anyone reading that function asks.

## Tests

`guard_rejection_is_an_event.rs` reshapes a refusal on each of the three transports into something a catcher chose, which is only reachable through the chain.

`method_enhancers::ws_method_level_enhancers_work` asserted the silence deliberately — *"guard should have silently blocked"* — and now asserts the envelope. The message after it still round-trips, so the connection is as usable as it was.
