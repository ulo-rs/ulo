# #6 — Add RPC client transport and enhance macro attribute detection

Merged 2026-03-22 into `master` from `feat/rpc-client`, commit [`96014be`](https://github.com/ulo-rs/ulo/commit/96014beb3c2efdab90d92ad954d2201e7093dc9f).

Introduce a client-side transport layer for RPC, enabling message sending capabilities alongside existing receiving functionality. Improve macro attribute detection to support path-qualified attributes, ensuring handlers are correctly recognized. Enhance the API for `RpcClient` and related structures, adding lifecycle management for client connections and improving usability for handler signatures.
