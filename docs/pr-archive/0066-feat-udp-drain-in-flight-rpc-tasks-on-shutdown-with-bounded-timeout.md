# #66 — feat(udp): drain in-flight RPC tasks on shutdown with bounded timeout

Merged 2026-05-02 into `master` from `feat/udp-drain`, commit [`e824286`](https://github.com/ulo-rs/ulo/commit/e824286b3e8b13ddb7d35af909cc97240335148c).

The UDP adapter previously stopped its recv loop on `close()` but spawned per-datagram tasks were left to run until completion or runtime drop — no graceful way to wait for in-flight handlers before tearing the process down. This PR mirrors the shape just landed for TCP so future RPC transports clone a hardened template.

A shared `JoinSet` now tracks every per-datagram task. On `close()`:

- The recv loop exits.
- The `JoinSet` is drained up to a configurable timeout (default 10 s).
- Anything still running after the timeout is aborted so `close()` never hangs.

Configure with `with_drain_timeout()`; pass `None` to wait without bound.

## Tests

- `udp_in_flight_request_completes_during_drain` — slow handler started before shutdown completes during the drain window.
- `udp_drain_aborts_after_timeout` — 50 ms drain budget vs 300 ms handler; shutdown completes promptly.
