# #98 — fix(redis-rpc): reconnect and resubscribe after a dropped connection

Merged 2026-06-11 into `master` from `feat/redis-reconnect`, commit [`6bdc4d9`](https://github.com/ulo-rs/ulo/commit/6bdc4d9019ed63510686c73f8f33061a6d858d81).

A dropped Redis connection left RPC silently dead until process restart: the pub/sub `on_message` stream ends on disconnect and nothing re-established it, so the server's channel subscriptions and the client's reply psubscription were gone. Redis pub/sub has neither auto-recovery nor an application-level heartbeat.

The serve loop and the client router now reopen the pubsub connection and resubscribe when the stream ends. Publishers move to `ConnectionManager`, which reconnects on its own, so only the pub/sub side needs the hand-rolled loop.

## Changes

- **Server** ([redis_adapter.rs](toni-redis-rpc/src/redis_adapter.rs)) — wrap subscribe + consume in a reconnect loop; reopen the pubsub and resubscribe every channel when the stream ends; publisher is now a `ConnectionManager`.
- **Client** ([redis_client_transport.rs](toni-redis-rpc/src/redis_client_transport.rs)) — the reply router reopens its pubsub and re-psubscribes the wildcard reply pattern when its stream ends; publisher is now a `ConnectionManager`.
- **Cargo** — enable the `connection-manager` redis feature.
- **Test** ([reconnect.rs](toni-redis-rpc/tests/reconnect.rs)) — force a real disconnect with `CLIENT KILL` (pausing the broker doesn't work — Redis pub/sub has no heartbeat, so a frozen-but-open socket is never noticed), then assert `send` recovers. Verified the test fails without the reconnect loop.
