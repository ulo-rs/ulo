# #69 — feat(rpc): wrap each request handler in a tracing span

Merged 2026-05-02 into `master` from `feat/rpc-tracing-spans`, commit [`17de971`](https://github.com/ulo-rs/ulo/commit/17de971c551e1f858b6d1ab7daa98baec1d5b2cd).

Without per-request spans, anything emitted from a handler — `tracing::info!` calls in user code, the framework's panic-catch error log, structured fields from a custom interceptor — surfaces with no way to correlate it back to the originating message. With dozens of in-flight requests on a single connection this makes operator debugging harder than it needs to be.

Each spawned handler task on TCP, and the post-parse body of `handle_datagram` on UDP, now runs inside an `rpc.request` span carrying:

- `transport` — `"tcp"` or `"udp"`
- `pattern` — the inbound message pattern
- `id` — the correlation id (Debug-formatted Option, so missing ids show as `None`)
- `peer` — the source address

Any tracing event emitted from the handler — adapter-level or user-level — automatically inherits those fields.

## Example

`examples/rpc_tracing.rs` runs both adapters side-by-side on a `LocalSet` with a `tracing-subscriber::fmt()` installed, so reviewers can see the span output end-to-end:

```
RUST_LOG=info cargo run --example rpc_tracing
```

The server-side log of a single request looks like:

```text
INFO rpc.request{transport="tcp" pattern=orders.create id=Some("req-1") peer=127.0.0.1:54321}: rpc_tracing: handler called item=keyboard qty=3
```

The handler itself just calls `tracing::info!(item, qty, "handler called")` — the four span fields are attached by the adapter.

## Notes

- No new dependencies in the adapters. `tracing` is already a workspace dep; the example pulls in `tracing-subscriber` (already a dev-dep elsewhere) and adds `tracing` to `examples/Cargo.toml` so user handler code can emit events.
- No configuration knobs. Spans are emitted unconditionally and surface only when the application installs a tracing subscriber.
- The error-frame emit paths (panic, handler error, write failure) keep their existing log calls; they now run inside the span so the operator sees `pattern=` / `id=` alongside.
- The UDP path required restructuring `handle_datagram` to wrap the post-parse body in an instrumented inner async block. The pre-parse path (JSON decode error) stays outside the span — there's no `pattern` / `id` to attach yet at that point. Worth noting in case reviewers wonder why one decode-error log line lacks the fields.

## Out of scope

Per-request **metrics** (counters / histograms) are explicitly *not* in this PR — they're a separate concern that needs an opinion on metrics backend (`metrics`, `prometheus`, OpenTelemetry, etc.) and will land as its own follow-up.
