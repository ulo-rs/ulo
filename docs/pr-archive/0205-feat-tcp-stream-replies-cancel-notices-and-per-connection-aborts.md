# #205 — feat(tcp): stream replies, cancel notices, and per-connection aborts

Merged 2026-08-29 into `master` from `feat/tcp-streaming`, commit [`2c68f6f`](https://github.com/ulo-rs/ulo/commit/2c68f6fac79f8e23cde811214a24156350200ce5).

Implements ADR-0032's stream grammar on TCP, the first transport to speak it end to end.

Server: a `Stream` answer drains through `wire::drive_reply_stream` — item frames and the end marker, each carrying the call's `id`, the shared writer locked per frame so a long drain does not starve the connection's other replies; a failed write stops the drain and fires the execution's token. Every request-shaped call registers in a per-connection `Inflight` before dispatch; an in-band `{"id","cancel":true}` frame aborts the driving task (mid-handler or mid-drain), and a dropped connection aborts everything the connection had in flight. Shutdown keeps its drain semantics — the drain timeout's abort is the backstop.

Client: the pending map becomes `PendingSlot::{Single, Stream}` — the single path is unchanged. Stream frames feed an `RpcReplyStream` through a per-call forwarder that enforces `with_timeout` as the frame-gap deadline (expiry yields `Err(Timeout)` and notifies the server); dropping the reply stream early writes the cancel frame. A single-reply answer to a `stream()` call is one item then the end; a `send()` answered with stream frames fails naming the mismatch.

Tests (`rpc_tcp_stream.rs`): frame order and the end marker with no `response` key on items; base64 `Binary` items decoding back through the client; both mid-stream error lanes (envelope item + clean end vs error end); the extension bag readable across the drain; and the producer observing cancellation on every abandonment path — cancel frame, dropped connection, dropped client stream, and a cancel mid-handler before any item, which drops the handler future. The PR 3 interim refusal test is superseded by the real grammar.

The full `rpc_tcp.rs` suite passes unchanged.
