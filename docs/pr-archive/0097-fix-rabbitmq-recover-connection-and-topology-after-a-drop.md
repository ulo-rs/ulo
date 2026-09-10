# #97 — fix(rabbitmq): recover connection and topology after a drop

Merged 2026-06-11 into `master` from `feat/rabbitmq-reconnect`, commit [`0d26a29`](https://github.com/ulo-rs/ulo/commit/0d26a2999048c08045997a024516d5fc60058450).

A dropped broker connection left RabbitMQ RPC silently dead until process restart: lapin cancels the consumers on disconnect, so the server's handler queues and the client's direct reply-to consumer stopped delivering.

The fix enables lapin's `enable_auto_recover`, which reconnects after a drop and replays topology — re-declaring the queues and resuming the consumers (server) and the direct reply-to consumer (client). The existing consume loops already `continue` on a recoverable error, so they keep yielding once lapin resumes the stream; no manual re-declaration is needed.

## Changes

- **Server** ([rabbitmq_adapter.rs](toni-rabbitmq/src/rabbitmq_adapter.rs)) — `enable_auto_recover()` on the connection.
- **Client** ([rabbitmq_client_transport.rs](toni-rabbitmq/src/rabbitmq_client_transport.rs)) — same on the connection backing the channel and direct reply-to consumer.
- **Test** ([reconnect.rs](toni-rabbitmq/tests/reconnect.rs)) — pauses the broker (cgroup freezer) long enough to trip a short `heartbeat` set in the URI, then unpauses, and asserts `send` recovers. Pausing keeps the published host port stable, which stop/start does not. Verified the test fails with auto-recover disabled.
