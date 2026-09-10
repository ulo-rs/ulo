# #212 — docs: show a streaming RPC reply and its cancellation by drop

Merged 2026-08-29 into `master` from `docs/rpc-streaming-example`, commit [`28b73f6`](https://github.com/ulo-rs/ulo/commit/28b73f654dd3acb216193b453d9a68c18f29a8fc).

Adds `examples/rpc_streaming.rs`: a TCP server with two streaming handlers — a bounded three-item stream collected by `RpcClient::stream`, and an unending tick stream whose producer watches `ctx.cancellation()` and stops when the client drops the reply stream. The example asserts the bounded items and runs under `cargo run --example rpc_streaming`.
