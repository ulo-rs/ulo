# #206 — feat(udp): stream replies, cancel datagrams, per-frame size checks

Merged 2026-08-29 into `master` from `feat/udp-streaming`, commit [`39bf5f1`](https://github.com/ulo-rs/ulo/commit/39bf5f12c567c0726079a5a543958f3332988d05).

Applies ADR-0032's stream grammar to UDP.

Server: the recv loop parses each datagram once and routes `{"id","cancel":true}` to an `Inflight` registry keyed `{source}|{id}`; request-shaped calls register before dispatch, so a cancel arriving in the next datagram aborts the driving task — mid-handler or mid-drain — and the execution's token fires. A stream answer is one datagram per item frame plus the end marker; an item too large for a datagram ends the stream with `{"end":true,"err":…}` rather than the single-reply path's logged drop, since a dropped frame mid-stream would read as loss. The adapter doc records the connectionless caveats: a lost `end` is bounded by the client's per-frame timeout, and a lost `cancel` leaves the producer running until its stream completes — the cancel datagram is the only abandonment signal the transport has.

Client: the pending map becomes `PendingSlot::{Single, Stream}` (single path unchanged); stream frames feed an `RpcReplyStream` through a per-call forwarder enforcing `with_timeout` as the frame-gap deadline, and an early drop sends the cancel datagram. Streams get no retries — a re-sent request could double-start the producer.

Tests (`rpc_udp_stream.rs`): frame order and the end marker, base64 `Binary` round-trip, the error end, the oversize-item loud end, producer cancellation via the cancel datagram and via client early-drop, single-handler compatibility, and the loud `send()` mismatch. The existing `rpc_udp.rs` suite passes unchanged.
