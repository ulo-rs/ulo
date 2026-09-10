# #45 — fix(rpc): catch handler panics and return error to caller (TCP + NATS)

Merged 2026-04-18 into `master` from `fix/rpc-handler-panic-recovery`, commit [`cdfe8ad`](https://github.com/ulo-rs/ulo/commit/cdfe8ad7eb789374419aa2c69ad4174ac71657a5).

## Summary

- Wraps each per-message handler call in `AssertUnwindSafe + catch_unwind` in both `TcpAdapter` and `NatsAdapter`
- Fire-and-forget messages that panic log the error and drop silently
- Request-response messages that panic immediately write an error frame back, so the caller unblocks within the handler's execution time rather than waiting out its own timeout

## Why

Without the fix a panicking handler kills its spawned task with nothing written to the wire. For TCP that means the caller reads no newline-delimited response and blocks until its timeout fires. For NATS no publish happens to the reply-to inbox, same result. This mirrors the WS panic recovery added in #44.

## Test plan

- [ ] `cargo test -p integration-tests --test integration rpc_panic_recovery` — new regression test; was failing before this fix (500 ms timeout hit, no response), passes after
- [ ] `cargo test -p integration-tests --test integration` — full suite, 130/130
