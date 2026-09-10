# #207 — feat(nats): stream replies to the inbox, cancel notices on one subject

Merged 2026-08-29 into `master` from `feat/nats-streaming`, commit [`4d46cf8`](https://github.com/ulo-rs/ulo/commit/4d46cf8c95a38c903577319a04433014877cc151).

Implements ADR-0032's stream grammar on NATS.

Server: a stream answer drains through `wire::drive_reply_stream`, publishing each frame to the call's reply inbox; the inbox is the correlation, so the grammar needs no new addressing. Request-shaped calls register in an `Inflight` keyed by inbox before dispatch. Cancel notices travel `toni.rpc.cancel` — subscribed without a queue group, so every instance sees every notice and the one holding the call aborts its driving task, firing the execution's token. A notice racing registration is dropped; the cancel channel is best-effort on a broker (recorded in ADR-0032).

Client: `send()`/`emit()` are untouched — the native `request()` stays. `open_stream` subscribes an explicit inbox, publishes the request with that reply subject, and a per-call forwarder feeds the caller's `RpcReplyStream`: `with_timeout` becomes the frame-gap deadline (expiry yields `Err(Timeout)` and publishes the cancel notice), an early drop publishes it too, and a single-reply answer is one item then the end.

Tests: `toni-nats/tests/round_trip.rs` is new (the crate had none), Docker-gated behind a new `integration` feature like the other broker crates — `send` round-trip, ordered stream items, and the producer observing the cancellation token after the client drops its stream. Passes against a live `nats:2.10` container.
