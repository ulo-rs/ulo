# #208 — feat(redis-rpc): stream replies on the reply channel, cancel notices on one channel

Merged 2026-08-29 into `master` from `feat/redis-streaming`, commit [`a768520`](https://github.com/ulo-rs/ulo/commit/a7685209542f3b68f08021e408b8c6516114561c).

Implements ADR-0032's stream grammar on Redis Pub/Sub.

Server: a stream answer drains through `wire::drive_reply_stream`, publishing each frame to the caller's `toni:rpc:reply:{client}:{corr}` channel — sequential publishes on the ConnectionManager keep them ordered. The dispatcher parses the request envelope in the receive loop and registers request-shaped calls in an `Inflight` keyed by reply channel before dispatch. Cancel notices travel `toni:rpc:cancel`, subscribed with the patterns and re-subscribed on every pubsub reconnect; the instance holding the call aborts its driving task, firing the execution's token. A notice racing registration is dropped — best-effort, as ADR-0032 records for brokers.

Client: the pending map becomes `PendingSlot::{Single, Stream}`; the reply router classifies each frame once (`parse_reply_frame`) and removes the entry on a terminal frame. A per-call forwarder feeds the `RpcReplyStream`, enforcing `with_timeout` as the frame-gap deadline; expiry and early drops publish the cancel notice. A `send()` answered with stream frames fails naming the mismatch.

Tests: `round_trip.rs` gains ordered stream items and the producer observing the cancellation token after a client-side drop, both against a live Redis container; the existing round-trip and reconnect suites pass unchanged.
