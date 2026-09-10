# #67 — test(rpc): drop retry-connect by reading the port from app.bind()

Merged 2026-05-02 into `master` from `feat/rpc-test-bind-readiness`, commit [`04ebc08`](https://github.com/ulo-rs/ulo/commit/04ebc08dff8979227e7e52993e509f8f796f68c3).

> Depends on #<rpc-bound-address PR>.

Now that `BoundAdapters` surfaces RPC adapter addresses, the integration tests can take the listener address directly from `app.bind().await` instead of pre-picking a port and retry-connecting until the spawned app finishes binding inside `app.start()`.

## What's gone

- `connect_with_retry` in `rpc_tcp.rs` — replaced by direct `TcpStream::connect`.
- `pick_free_port` / `pick_free_udp_port` helpers — no longer needed; binding to port 0 and reading `bound.rpc.unwrap().port()` gives an OS-assigned port that's already live.
- The 50 ms sleep before constructing `UdpClientTransport` in `udp_client_transport_round_trips_and_rejects_oversized` — same readiness gap, same fix.

## What stays

The send-retry inside `udp_rpc_timeout` is kept. It's defensive against kernel queue pressure on the local socket — not bind timing — and removing it would make the helper less robust without saving anything meaningful. Comment updated to reflect the actual reason.
