# #210 — feat(mqtt): stream replies on the response topic, cancel notices on one topic

Merged 2026-08-29 into `master` from `feat/mqtt-streaming`, commit [`1ed1e2d`](https://github.com/ulo-rs/ulo/commit/1ed1e2d57e9c5c9e069f4cacab5aa66b7438f124).

Implements ADR-0032's stream grammar on MQTT v5.

Server: a stream answer drains through `wire::drive_reply_stream`, publishing each frame (QoS 1) to the request's `response_topic` with `correlation_data` echoed. Request-shaped publishes register in an `Inflight` keyed by correlation data before dispatch. Cancel notices travel `toni/rpc/cancel`, subscribed on every ConnAck with the pattern topics (rumqttc does not replay subscriptions); the instance holding the call aborts its driving task, firing the execution's token.

Client: correlation ids become `{reply_topic}:{n}` — the reply topic is unique per client, and the server's cancel registry is shared by every caller, so bare counters would collide. The pending map splits into `PendingSlot::{Single, Stream}`; the event-loop router classifies frames once, a per-call forwarder enforces `with_timeout` as the frame-gap deadline, and expiry or an early drop publishes the cancel notice. A `send()` answered with stream frames fails naming the mismatch.

Tests: `round_trip.rs` gains ordered stream items and the producer observing the cancellation token after a client-side drop, against a live Mosquitto container; the existing round-trip and broker-restart reconnect suites pass unchanged.
