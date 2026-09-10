# #211 — feat(kafka): stream replies keyed for order, cancel notices on one topic

Merged 2026-08-29 into `master` from `feat/kafka-streaming`, commit [`654f652`](https://github.com/ulo-rs/ulo/commit/654f652d2d6bef0de40de0eb42aef894f0d548a5).

Implements ADR-0032's stream grammar on Kafka.

Server: a stream answer drains through `wire::drive_reply_stream`, producing one record per frame to the caller's reply topic — every record keyed by the correlation id, so the whole stream lands on one partition and stays ordered (the test pins 50 items in order across a live broker). Request-shaped records register in an `Inflight` keyed by correlation id before dispatch. Cancel notices travel `toni.rpc.cancel`, consumed in a unique per-instance group: the pattern group load-balances partitions, but every instance must see every notice because only the one holding the call can act. `ensure_topics` pre-creates the cancel topic with the pattern topics.

Client: correlation ids become `{reply_topic}:{n}` — the reply topic is unique per client, and the server's cancel registry is shared by every caller, so bare counters would collide. The pending map splits into `PendingSlot::{Single, Stream}`; the reply router classifies frames once, a per-call forwarder enforces `with_timeout` as the frame-gap deadline, and expiry or an early drop produces the cancel notice. A `send()` answered with stream frames fails naming the mismatch.

Tests: `round_trip.rs` gains 50 ordered stream items (the partition-ordering proof) and the producer observing the cancellation token after a client-side drop, against a live Kafka container; the existing round-trip suite passes unchanged.
