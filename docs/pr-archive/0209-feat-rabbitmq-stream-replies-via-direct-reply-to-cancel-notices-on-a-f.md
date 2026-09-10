# #209 — feat(rabbitmq): stream replies via direct reply-to, cancel notices on a fanout

Merged 2026-08-29 into `master` from `feat/rabbitmq-streaming`, commit [`46f33e6`](https://github.com/ulo-rs/ulo/commit/46f33e6f51f36d992bd808399cd92af3e10d5dbf).

Implements ADR-0032's stream grammar on RabbitMQ.

Server: a stream answer drains through `wire::drive_reply_stream`, publishing each frame to the caller's reply queue with the request's correlation id echoed — direct reply-to accepts any number of publishes while the reply consumer lives. Request-shaped deliveries register in an `Inflight` keyed by correlation id before dispatch. Cancel notices travel a fanout exchange, `toni.rpc.cancel`, into a named per-instance auto-delete queue on a dedicated channel — a server-named queue would break lapin's topology replay after a reconnect, and a failed replay closes its channel, which must not take the pattern consumers down (the broker-freeze reconnect test pins this).

Client: correlation ids become `{client_id}:{n}` — the server's cancel registry is shared by every caller, so bare counters would collide across clients and one caller's cancel could abort another's call. The pending map splits into `PendingSlot::{Single, Stream}`; the reply router classifies frames once, a per-call forwarder enforces `with_timeout` as the frame-gap deadline, and expiry or an early drop publishes the cancel notice. The client declares the fanout before use — publishing to a missing exchange closes the channel. A `send()` answered with stream frames fails naming the mismatch.

Tests: `round_trip.rs` gains ordered stream items and the producer observing the cancellation token after a client-side drop, against a live RabbitMQ container; the existing round-trip and broker-freeze reconnect suites pass unchanged.
