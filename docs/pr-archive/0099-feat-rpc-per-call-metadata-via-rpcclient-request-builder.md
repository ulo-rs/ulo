# #99 — feat(rpc): per-call metadata via RpcClient::request() builder

Merged 2026-06-11 into `master` from `feat/rpc-client-metadata`, commit [`ddef92b`](https://github.com/ulo-rs/ulo/commit/ddef92b1896b1a776de13428bb3ade18077dad53).

`RpcClient` could attach no per-call metadata (auth tokens, trace ids, tenant) to a request — metadata only flowed server-inbound, read off the wire by the adapters. This adds a client-side builder so callers can set it:

```rust
client.request("inventory.restock")
    .metadata("trace-id", trace_id)
    .metadata("tenant", tenant)
    .send(RpcData::json(payload))
    .await?;
```

`RpcClientTransport::send`/`emit` now carry a `HashMap<String,String>`; the plain `RpcClient::send`/`emit` (and `*_json`) shorthands pass an empty map and are unchanged for callers. Each transport places the entries on its native side channel.

## Components

- **Core** ([rpc_client_transport.rs](toni/src/adapter/rpc_client_transport.rs), [rpc_client.rs](toni/src/rpc/rpc_client.rs)) — metadata on the trait methods; `RpcClient::request()` returning an `RpcRequest` builder with `.metadata()`, `.send()`/`.emit()` and `.send_json()`/`.emit_json()`.
- **Transports** — NATS/RabbitMQ/MQTT carry metadata in native headers (NATS headers, AMQP headers, MQTT v5 user properties); Redis/TCP/UDP carry it in the request envelope.
- **Tests** — the broker round-trip tests now exercise the builder end-to-end (replacing the raw-publish stand-ins), proving client → handler `RpcContext` metadata.

## Deferred to a follow-up

- **TCP/UDP server-side read.** TCP and UDP now *send* a `metadata` envelope field, but their servers don't yet parse it into `RpcCallInfo` (the brokers and NATS already do). Closing that — and the long-standing TCP/UDP metadata gap — is the immediate next PR.
