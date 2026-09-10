# #93 — feat(redis-rpc): Redis Pub/Sub RPC transport

Merged 2026-06-09 into `master` from `feat/redis-rpc-adapter`, commit [`a893b52`](https://github.com/ulo-rs/ulo/commit/a893b52fb68757c92dce3f06aea91cd153829f29).

Adds `toni-redis-rpc`, an RPC transport over Redis Pub/Sub — the first broker transport since NATS.

Redis Pub/Sub has no native request-reply, so request-response is emulated: each `send` publishes a correlation-keyed reply channel inside a JSON envelope, and a single background subscription routes replies back to the waiting caller (no per-request connection churn). The same envelope carries per-call `metadata`, since Pub/Sub frames have no headers of their own — so `RpcContext::metadata()` is populated end to end.

## Components

- **Wire** ([wire.rs](toni-redis-rpc/src/wire.rs)) — request envelope (`data` / `reply_to` / `metadata`) plus response framing and parsing, mirroring the `toni-nats` `{"response"}` / `{"err"}` convention.
- **Server** ([redis_adapter.rs](toni-redis-rpc/src/redis_adapter.rs)) — `RedisAdapter`: subscribes one channel per registered pattern, copies envelope metadata into `RpcCallInfo`, dispatches, and publishes the framed reply to the caller's channel. Retries the initial connect (~10s) like the NATS adapter rather than failing fast, since a broker may start slowly.
- **Client** ([redis_client_transport.rs](toni-redis-rpc/src/redis_client_transport.rs)) — `RedisClientTransport`: lazy connect, one background reply-router wildcard-subscribed to `toni:rpc:reply:{id}:*`, correlation routing via per-call oneshots.
- **Tests** — wire round-trip unit tests (no Docker) and a live-Redis integration test behind the `integration` feature (testcontainers) covering send, emit, and metadata.

## Deferred to follow-ups

- No reconnect if the Pub/Sub connection drops (adapter stream / client router end silently).
- Patterns map to exact Redis channels only — no glob (`PSUBSCRIBE`).
- The client `send` / `emit` API doesn't yet expose setting metadata; the server-side path is proven via raw publish in the integration test.
