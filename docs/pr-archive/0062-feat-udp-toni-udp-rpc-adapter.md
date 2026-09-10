# #62 — feat(udp): toni-udp RPC adapter

Merged 2026-05-01 into `master` from `feat/rpc-udp`, commit [`a1a4e2a`](https://github.com/ulo-rs/ulo/commit/a1a4e2acb064a2e1ee4bb515aab22b2596339f40).

## Summary

Adds `toni-udp`, a UDP transport adapter for the Toni RPC gateway. Same JSON envelope as `toni-tcp` (one datagram = one message, no newline framing).

- `UdpAdapter` (server): single `UdpSocket`, recv loop with `tokio::select!` against a `watch::Sender<bool>` so `RpcAdapter::close` cleanly drives the loop to exit. Bind happens synchronously so port collisions surface as `Err` at startup instead of panicking inside the spawned future. Panic recovery + structured error envelopes match the TCP adapter.
- `UdpClientTransport` (client): monotonic correlation id, background reader for demux, lazy reconnect (the cached socket is dropped on recv/send error so the next call rebuilds), and opt-in retries-with-backoff on `send` timeout via `with_retries(n)` + `with_retry_backoff(Duration)`. Default `retries = 0` — no behaviour change for naive use.
- `examples/rpc_udp.rs` mirrors the existing TCP example so users can compare side-by-side.

Deferred to a follow-up: length-prefixed framing for >65 KiB payloads. That's a wire-format change with its own design questions (chunk size, reassembly timeouts, memory-DoS surface), worth reviewing on its own.

## Test plan

- [x] `cargo build -p toni-udp`
- [x] `cargo test -p toni-udp --lib` — reconnect path unit test passes
- [x] `cargo test -p integration-tests --test integration rpc_udp` — 7 integration tests pass: round-trip, unknown-pattern, panic recovery, fire-and-forget, oversized rejection, graceful shutdown, retry-on-loss
- [x] `cargo build --example rpc_udp`
