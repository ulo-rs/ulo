# #101 — feat(kafka): Apache Kafka RPC transport

Merged 2026-06-12 into `master` from `feat/kafka-rpc-adapter`, commit [`898aba4`](https://github.com/ulo-rs/ulo/commit/898aba4dbdf01b2753a83df9a7f3ad21330bbb3c).

Adds `toni-kafka`, an RPC transport over Apache Kafka.

Kafka is an event log with no native request-response, but it carries message headers. So per-call metadata and the reply addressing ride headers (no envelope), and request-response is emulated: a request names a private reply topic and a correlation id in its headers, the server produces the reply there, and the client routes replies back by correlation id. A pattern maps to a Kafka topic.

## Components

- **Wire** ([wire.rs](toni-kafka/src/wire.rs)) — payload <-> bytes; build/read headers (`toni-reply-to`, `toni-correlation-id`, and user metadata, skipping the control keys); response framing/parsing matching the other RPC transports; `ensure_topics` (admin-client pre-creation).
- **Server** ([kafka_adapter.rs](toni-kafka/src/kafka_adapter.rs)) — `KafkaAdapter`: a `StreamConsumer` (stable group, `auto.offset.reset=latest`) subscribed to the pattern topics; copies headers into `RpcContext` metadata; replies via a `FutureProducer`. `with_group_id` configures the group.
- **Client** ([kafka_client_transport.rs](toni-kafka/src/kafka_client_transport.rs)) — `KafkaClientTransport`: a producer plus one consumer on a private reply topic (unique group, `earliest`), correlation-routed.
- **Tests** — wire/header unit tests (no Docker) and a live-broker integration test behind the `integration` feature (testcontainers) covering send, emit, and header metadata.

## Notes

- The adapter and client **pre-create their topics** via an admin client so consumers assign partitions at join, rather than waiting for a metadata refresh to discover an auto-created topic (which made fire-and-forget delivery slow/flaky).
- librdkafka builds from the vendored C source via `make` (no cmake, no system library).

## Deferred to follow-ups

- No reconnect/topology re-setup beyond what librdkafka does internally; consumer-group rebalancing is left to the client library.
- One partition per topic, replication factor 1 — no partition/RF/retention tuning surfaced.
- Client `send`/`emit` carry metadata; no consumer-group or offset-commit policy knobs exposed.
