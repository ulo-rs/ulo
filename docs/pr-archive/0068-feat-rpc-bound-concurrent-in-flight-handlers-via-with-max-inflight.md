# #68 — feat(rpc): bound concurrent in-flight handlers via with_max_inflight

Merged 2026-05-02 into `master` from `feat/rpc-backpressure`, commit [`cb08b63`](https://github.com/ulo-rs/ulo/commit/cb08b63469d3405020277f96a90354b56128e703).

Both TCP and UDP adapters previously spawned one task per inbound message with no upper bound. A misbehaving (or hostile) client could exhaust memory or file descriptors before the operator noticed. This adds an opt-in cap so the canonical RPC adapter shape — bind synchronously, surface local_addr, drain on close, bounded in-flight — is complete before we clone it into gRPC, Kafka, MQTT, RabbitMQ, and Redis.

## API

`adapter.with_max_inflight(n)` installs a shared `Semaphore` with `n` permits. Each inbound message tries to acquire a permit before spawning the handler. When at capacity:

- **Request-response** (frame has `id`): caller gets an `"overloaded"` error frame back immediately. Liveness preserved — the caller knows to back off rather than queuing forever.
- **Fire-and-forget**: dropped with a log line. There's no caller to notify.

Permits are released when the spawned task completes or is aborted.

Default: unbounded (opt-in feature, no behavior change for current users).

## Tests

- `tcp_backpressure_rejects_excess_and_releases_after_completion` — cap=1, slow handler holds the slot, second request rejected with `"overloaded"`, third (after slot released) succeeds.
- `udp_backpressure_rejects_excess_and_releases_after_completion` — same shape.

## Drive-by fix

The merge of #65 (test refactor) onto the drain branches left the drain tests calling `pick_free_port` / `pick_free_udp_port` helpers that #65 had deleted — `cargo test` on master no longer compiled. Repaired those tests to use the bind-readiness pattern consistent with the rest of the suite.
