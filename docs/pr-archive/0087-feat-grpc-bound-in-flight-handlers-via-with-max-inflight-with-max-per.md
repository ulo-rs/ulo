# #87 — feat(grpc): bound in-flight handlers via with_max_inflight + with_max_per_connection

Merged 2026-05-30 into `master` from `feat/grpc-backpressure`, commit [`9915c47`](https://github.com/ulo-rs/ulo/commit/9915c470225793f3222eabaef8da7722482c303a).

## Summary

The TCP and UDP adapters expose `with_max_inflight(usize)` so a misbehaving (or hostile) client can't pin server memory by spawning unlimited concurrent calls; the gRPC adapter had nothing. This PR closes that gap with two knobs — a global cap and a per-connection cap — so the canonical adapter shape (bind + local_addr + drain on close + bounded in-flight + per-request tracing) is now symmetric across HTTP / RPC / gRPC and a reviewer asking about rate-limiting on a new transport gets a consistent answer.

## Knobs

Both ship together because they cover different cases and adding one then the other would force users to re-tune:

- **`with_max_inflight(impl Into<Option<usize>>)`** — global concurrency cap across all connections. Protects total memory. Built on `tower::limit::GlobalConcurrencyLimitLayer`.
- **`with_max_per_connection(impl Into<Option<usize>>)`** — per-connection cap. Prevents one slow client from monopolising the server even when the global cap isn't hit. Built on tonic's `concurrency_limit_per_connection`.

`load_shed(true)` is set alongside either cap so at-cap requests surface as `Status::resource_exhausted` immediately rather than queueing. Without it, the cap just delays things and the OOM-protection point is lost.

## Implementation detail

The global cap rides on `tower::limit::GlobalConcurrencyLimitLayer` applied *unconditionally* via `tower::util::option_layer(max_inflight.map(...))` — without that wrapper, conditional `.layer()` calls produce different concrete `Server` types and the chained builder can't reassign. The per-connection cap uses tonic's built-in `concurrency_limit_per_connection`, which is a `Self` → `Self` mutator and chains cleanly.

`tower` now depends on `"util"` *and* `"limit"` features.

## Tests

One new integration test in `rpc_grpc_macros.rs`, mirroring the TCP backpressure test:

- Configure `with_max_inflight(1)`.
- Fire two concurrent slow calls on separate channels (so a single connection's HTTP/2 multiplexing can't perturb the global-cap measurement).
- Assert the second call gets `tonic::Code::ResourceExhausted` immediately rather than queueing.
- Wait for the first call to finish, fire a third call — assert it succeeds, proving the permit released cleanly.

201/201 integration tests pass.

`with_max_per_connection` is not exercised by a dedicated test for now — same fixture would work but the assertion is per-connection rather than global, and adding another slow-handler module to exercise only that knob is more boilerplate than payoff at this point. Revisit if a user reports the per-connection cap not behaving as expected.

## Test plan

- [x] `cargo build --workspace --all-targets` — clean.
- [x] `cargo test -p integration-tests --test integration` — 201/201.
- [x] `cargo test -p integration-tests --test integration grpc_backpressure` — passes.
