# #95 — feat(mqtt): MQTT v5 RPC transport

Merged 2026-06-10 into `master` from `feat/mqtt-rpc-adapter`, commit [`4ca9f5e`](https://github.com/ulo-rs/ulo/commit/4ca9f5e0dd282589cbd17388a7787e9f6be4e900).

Adds `toni-mqtt`, an RPC transport over MQTT v5.

MQTT v5 carries request-response natively: a PUBLISH can name a `response_topic` and `correlation_data`, and per-call metadata rides in the v5 user properties. The body is raw `RpcData` bytes, addressing lives in the PUBLISH properties, and metadata maps to user properties. Client and server each drive a rumqttc event loop — the loop is what transmits queued publishes and delivers incoming messages.

## Components

- **Wire** ([wire.rs](toni-mqtt/src/wire.rs)) — payload <-> bytes, v5 user properties -> metadata, and response framing/parsing matching the `{"response"}` / `{"err"}` convention shared by the RPC transports.
- **Server** ([mqtt_adapter.rs](toni-mqtt/src/mqtt_adapter.rs)) — `MqttAdapter`: subscribes one topic per pattern (exact-topic match), copies user properties into `RpcContext` metadata, dispatches, and publishes the framed reply to the request's `response_topic` with `correlation_data` echoed back. The poll loop retries on connection error (rumqttc reconnects on the next poll).
- **Client** ([mqtt_client_transport.rs](toni-mqtt/src/mqtt_client_transport.rs)) — `MqttClientTransport`: lazy connect, one private reply topic, correlation routing via per-call oneshots, and a wait for the reply-topic SubAck before the first send so a reply cannot arrive ahead of its route.
- **Tests** — wire/user-property unit tests (no Docker) and a live-broker integration test behind the `integration` feature (testcontainers mosquitto) covering send, emit, and user-property metadata.

## Deferred to follow-ups

- No resubscribe after a reconnect — rumqttc reconnects the socket but queued subscriptions are not replayed, so handlers go quiet until restart if the broker connection drops.
- Exact-topic match only; MQTT topic wildcards (`+`/`#`) are not mapped to patterns.
- The client `send` / `emit` API doesn't yet expose setting user properties; the server-side metadata path is proven via raw publish in the integration test.
