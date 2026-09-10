# #9 — Support NATS cluster addresses

Merged 2026-03-23 into `master` from `feat/rpc-nats-cluster`, commit [`f8f412f`](https://github.com/ulo-rs/ulo/commit/f8f412f328585c27aab83defbc4a94e35e36cd71).

Enable support for multiple NATS server addresses to improve adapter usability with NATS clusters. Introduce a new trait for flexible server address handling, allowing single URLs or collections. Adjust the NatsAdapter and NatsClientTransport to utilize this new functionality.
