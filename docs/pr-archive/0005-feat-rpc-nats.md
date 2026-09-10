# #5 — Feat/rpc nats

Merged 2026-03-21 into `master` from `feat/rpc-nats`, commit [`31fa03a`](https://github.com/ulo-rs/ulo/commit/31fa03aaad3e1032d21eba50d4ab2084cefb8a9e).

TCP reads the handler pattern from a message envelope, but pub-sub transports like NATS route by subject — the subject IS the pattern. Subscribing per-pattern at startup eliminates the envelope and lets the broker handle routing.

Startup tolerates NATS not being ready: the adapter retries in the background so the HTTP server isn't blocked. Connection state is logged only when the server is actually reachable, not on client construction, so the log is trustworthy.
