# #58 — test(integration): make integration suite nextest-safe via port 0

Merged 2026-04-28 into `master` from `refactor/test-port-isolation`, commit [`7aaaed6`](https://github.com/ulo-rs/ulo/commit/7aaaed6fdbd9db7e061844541d5a964c2b2585cc).

The integration suite was wedged on a static `AtomicU16` port counter
plus a 500ms readiness sleep — a pattern that worked under `cargo test`
because the counter was per-process state, but broke under nextest's
process-per-test isolation: every test process resets the counter to
its starting value, and concurrent processes raced for the same port.

The recent ServerHandle refactor (#56) gave the framework everything
needed to fix this — `app.bind()` now resolves only after the socket is
live and surfaces the OS-assigned address via `BoundAdapters::http`.
This branch finishes the job test-side:

- HTTP bypass files (`lifecycle_hooks`, `error_handler`) move from
  `app.start()` to `app.bind()` + port 0; the per-file static port
  counter and sleep both go.
- RPC bypass files (`method_enhancers`, `rpc_panic_recovery`) can't use
  `BoundAdapters` — `TcpAdapter` binds inside its serve future. They
  pre-bind a listener on `127.0.0.1:0` to claim a port, drop it, and
  hand the port to the adapter, then replace the readiness sleep with
  a connect-retry loop.
- `#[serial]` sweep across 27 files: removed where the only reason for
  serialization was port-counter contention. Kept on the 11 files with
  genuine cross-test global state (env vars, trackers, lifecycle log).
