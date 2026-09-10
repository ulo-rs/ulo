# #213 — docs(adr): mark ADR-0032 accepted

Merged 2026-08-29 into `master` from `docs/adr-0032-accepted`, commit [`2d88294`](https://github.com/ulo-rs/ulo/commit/2d882949eb078f7b157fc8d6e5cda18d809ee1ca).

Flips ADR-0032's status: the decision is implemented end to end — the output enum, the wire grammar, the cancel routing, and the client verbs are live on all seven transports (#202–#211), with `examples/rpc_streaming.rs` as the runnable reference (#212).
