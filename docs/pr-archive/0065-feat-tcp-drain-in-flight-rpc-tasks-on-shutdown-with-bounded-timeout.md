# #65 — feat(tcp): drain in-flight RPC tasks on shutdown with bounded timeout

Merged 2026-05-02 into `master` from `feat/tcp-drain`, commit [`ef6fa62`](https://github.com/ulo-rs/ulo/commit/ef6fa6242784092e607f7d2e2d3b2f9aa0f9f738).

The TCP adapter previously stopped its accept loop on `close()` but spawned per-message tasks were left to run until they finished, the client disconnected, or the runtime was dropped. There was no graceful way for an operator to wait for in-flight handlers before tearing the process down — fine for v1, but the framework is about to grow several more RPC transports and they should clone a hardened shape, not the v1 gap.

A shared `JoinSet` now tracks every spawned task (connection handlers and per-message tasks). On `close()`:

- The accept loop exits.
- Connection handlers stop reading new lines (they race `read_line` against the shutdown signal).
- The `JoinSet` is drained up to a configurable timeout (default 10 s).
- Anything still running after the timeout is aborted so `close()` never hangs.

The drain budget is configurable per-adapter via `with_drain_timeout()`, and passing `None` disables the timeout for callers who want to wait without bound.

## Tests

- `tcp_in_flight_request_completes_during_drain` — a slow handler started before shutdown completes successfully during the drain window.
- `tcp_drain_aborts_after_timeout` — a request whose handler outruns a 50 ms drain budget gets aborted; `shutdown.completed()` resolves promptly.
