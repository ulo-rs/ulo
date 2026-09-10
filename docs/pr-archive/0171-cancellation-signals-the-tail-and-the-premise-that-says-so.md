# #171 — Cancellation signals the tail, and the premise that says so

Merged 2026-08-22 into `master` from `docs/adr-cancellation`, commit [`92e0cb0`](https://github.com/ulo-rs/ulo/commit/92e0cb019d45ac7f5ab6a6f0b5f25a9134ae5355).

Records what a cancellation token is for in a language where futures already stop when dropped, and pins the premise that argument rests on.

`CancellationToken` is complete — `cancel`, `is_cancelled`, and an async `cancelled` for `select!` — and sits on all four contexts. Nothing has ever fired it, nothing reads it, and the `Cancelled` event names a producer in its own doc comment that it has never had.

**The reason nothing missed it.** Nest needs an `AbortSignal` because a Node promise keeps running after the socket closes. A Rust future stops when dropped, and the server drops the response future when the connection dies. The tests here pin both halves:

| | |
| --- | --- |
| `a_disconnect_drops_the_handler_future` | client goes away → the handler's sentinel drops |
| `a_live_connection_does_not_drop_the_handler` | client still waiting → sentinel intact |

The second is what makes the first mean anything, and it is easy to build wrong: a control written with `timeout` drops the request future, which closes the connection, and so controls for nothing. The one here holds the request future across a `select!` instead.

**What a dropped future does not reach** is work that escaped the handler: the streaming tail, where the body is draining and a task feeding it learns only at its next send, and deliberately detached work holding a context clone. ADR 0016 made both part of the execution, which is what makes a producer possible.

The decision:

- `ScopedBody` and `ScopedStream` track completion, and a `Drop` before the end fires the token — a body dropped before its last frame being the disconnect signal.
- Nothing else produces it. A buffered response has nothing alive that could hear it.
- `Cancelled` stays unproduced, recorded as a gap with its reason: `Drop` is synchronous, `ErrorObserver::observe` is not, and core holds no runtime handle.
- `deadline()` from `grpc-timeout` is a separate feature and stays unimplemented.

Roads not taken are recorded, including the Nest-shaped one — a signal on every disconnect spends itself on the case the language already handles.

The implementation follows separately. The tests are here because the decision's central claim is empirical and this is its proof.
