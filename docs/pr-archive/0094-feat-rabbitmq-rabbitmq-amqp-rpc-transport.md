# #94 — feat(rabbitmq): RabbitMQ (AMQP) RPC transport

Merged 2026-06-10 into `master` from `feat/rabbitmq-rpc-adapter`, commit [`b273d40`](https://github.com/ulo-rs/ulo/commit/b273d4072461d58a1793a42fd29b513729d3b99a).

Adds `toni-rabbitmq`, an RPC transport over RabbitMQ (AMQP 0-9-1).

AMQP carries request-response natively: a message has `reply_to`, `correlation_id`, and a headers table. The body is raw `RpcData` bytes, addressing lives in the message properties, and per-call metadata maps to AMQP headers. The client uses RabbitMQ direct reply-to (`amq.rabbitmq.reply-to`), so request-response needs no dedicated reply queue.

## Components

- **Wire** ([wire.rs](toni-rabbitmq/src/wire.rs)) — payload <-> bytes, AMQP headers -> metadata, and response framing/parsing matching the `{"response"}` / `{"err"}` convention shared by the RPC transports.
- **Server** ([rabbitmq_adapter.rs](toni-rabbitmq/src/rabbitmq_adapter.rs)) — `RabbitMqAdapter`: declares one queue per pattern (routed via the default exchange), consumes, copies headers into `RpcContext` metadata, dispatches, and publishes the framed reply to the delivery's `reply_to` with the matching `correlation_id`. Retries the initial connect (~10s).
- **Client** ([rabbitmq_client_transport.rs](toni-rabbitmq/src/rabbitmq_client_transport.rs)) — `RabbitMqClientTransport`: lazy connect, one direct reply-to consumer, correlation routing via per-call oneshots.
- **Tests** — wire/header unit tests (no Docker) and a live-broker integration test behind the `integration` feature (testcontainers) covering send, emit, and header metadata.

lapin 4 wires the tokio runtime itself (default `ConnectionProperties`), so no executor/reactor glue crates are needed.

## Deferred to follow-ups

- No reconnect if the AMQP connection drops (adapter consumers / client router end silently).
- Queues use default options (transient, default-exchange routing); no durability/QoS/prefetch knobs exposed.
- The client `send` / `emit` API doesn't yet expose setting headers; the server-side metadata path is proven via raw publish in the integration test.
