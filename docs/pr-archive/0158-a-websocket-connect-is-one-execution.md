# #158 — A WebSocket connect is one execution

Merged 2026-08-19 into `master` from `feat/ws-one-execution-per-message`, commit [`47ac89f`](https://github.com/ulo-rs/ulo/commit/47ac89fb0f7c07be96ffca43d7c95262dd65541b).

The guards that admit a WebSocket connection and the `#[on_connect]` hook that greets it share one context, so what a guard writes is what the hook reads. Completes ADR 0016.

The connect path built three bags across a single connect: one context for the guards, a stored client whose bag was a different one, and a third context for the hook. A connect guard's write reached none of them — the hook reads the bag through the client it is handed, and that client had never been pointed at the guards' bag. Nothing covered it.

`begin_connect` returns the context it built and `complete_connect` finishes it. The two phases stay split because the adapter registers the client's sink between them, which is a sequencing need rather than a second execution.

A message remains its own execution and still starts with an empty bag.

## Not connection scope

State that spans a connection is session state, and this does not add any. `WsClient::extensions` documents the boundary it now has: scoped to one execution, with nothing written at connect surviving into the first message. ADR 0016 leaves the session scope for its own decision, and authenticating at connect to read the identity per message still has no home.

Nest reaches the opposite arrangement by keying its context id on the client object, which memoises the id there and makes the boundary the connection. ADR 0016 records why that is not copied: it is a session scope wearing an execution scope's machinery.

## Public surface

`GatewayWrapper::begin_connect` returns `WsContext` instead of `()`, and `complete_connect` takes `&WsContext` instead of a client id. Both are called from toni core only; no adapter sees them, and the WebSocket callback contract is unchanged.

## Tests

One test pins both sides of the boundary: a connect guard's write reaches the hook, and does not reach a message handler. The guard stamps only when the event is `connect`, because a gateway-level guard also runs per message and stamping there would leave a leak indistinguishable from a fresh write. Restoring the third context fails the first assertion with the original symptom.

## Status

ADRs 0016 and 0017 move from `proposed` to `accepted`. Every consequence 0016 lists is in the tree.
