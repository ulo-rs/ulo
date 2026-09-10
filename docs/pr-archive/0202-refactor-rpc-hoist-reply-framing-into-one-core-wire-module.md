# #202 — refactor(rpc): hoist reply framing into one core wire module

Merged 2026-08-29 into `master` from `refactor/rpc-wire-module`, commit [`a53d98a`](https://github.com/ulo-rs/ulo/commit/a53d98ad905c91fa671ddaca13e12f9057f8fcfb).

Hoists the RPC reply framing into one core module, `toni::rpc::wire`: `frame_response`, `frame_panic`, and `parse_response` now exist once, used by all seven server adapters and the envelope-parsing client transports. `ResponseFrame` carries the two carrier forms — `into_bytes` with raw `Binary` passthrough (the brokers), `into_json_value` degrading `Binary` to a null response (tcp/udp, which splice `"id"` into the frame). The broker crates' `wire.rs` modules keep only their transport-specific helpers; the tcp/udp/nats inline copies and every per-crate `error_status` are gone.

No wire change: the module's tests assert the frame bytes per outcome arm (json/text/binary/ack/AppError/wire-err/panic), and the framing tests that lived in the broker crates now run in core.
