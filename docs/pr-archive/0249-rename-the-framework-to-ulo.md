# #249 — Rename the framework to ulo

Merged 2026-09-10 into `master` from `rename/the-framework-is-named-ulo`, commit [`903b242`](https://github.com/ulo-rs/ulo/commit/903b242093d311abb26b66c6667c74f8c46ab5bb).

The framework is `ulo`, and every integration crate names its role before its library. ADR-0044 records the decision.

- **Names** — `ulo`, `ulo-macros`, `ulo-build`, `ulo-cli`, `ulo-config`, `ulo-grpc` and `ulo-health` stay bare; integrations are `ulo-<role>-<library>` across `http` (5), `rpc` (7), `db` (6), `ws` (2) and `graphql` (2). `toni-redis`, `toni-redis-rpc` and `toni-redis-broadcast` were a database pool, an RPC transport and a WebSocket backplane behind one middle word; they are `ulo-db-redis`, `ulo-rpc-redis` and `ulo-ws-redis`.
- **Layout** — the 29 crates move to a flat `crates/`. The role prefixes sort them into families, and every inter-crate path dependency stays `../<name>` at unchanged depth.
- **CLI** — the binary is `ulo`, so the commands are `ulo new`, `ulo generate resource` and `ulo dev`.
- **ADRs** — the 43 existing ADRs have their `toni::` paths rewritten to match the code they explain. None of them is a decision about the name.
- **LICENSE** — the upstream copyright line is kept, as MIT requires, with this fork's beneath it.
- **Repository URLs** — `repository` and `homepage` still name `toni-rs`, the repository that exists.

Deferred to a follow-up:

- Renaming the GitHub repository: it is outside this tree.
- Updating the docs site: it is a separate repository.
