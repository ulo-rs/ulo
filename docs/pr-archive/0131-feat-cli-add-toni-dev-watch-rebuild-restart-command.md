# #131 — feat(cli): add toni dev watch/rebuild/restart command

Merged 2026-07-29 into `master` from `feat/cli-dev-command`, commit [`17442c0`](https://github.com/ulo-rs/ulo/commit/17442c0fdecc3b3cd828a405173d213e9152c790).

Adds `toni dev`: runs the application and restarts it on every `.rs`/`.toml` save, with `.gitignore` paths and `target/` excluded.

The watcher is the watchexec library — the engine behind the watchexec CLI — so file watching, debouncing, ignore handling, and process supervision are reused rather than reimplemented, and no external tool install is needed. The application is spawned in its own process group: `cargo run` puts the real process a level down, and group signalling is what lets stop and restart reach it. `--release` builds in release mode; everything after `--` is forwarded to the application binary. Compile failures print and leave the watcher waiting for the next save.

The command sits behind a default-on `dev` feature; `cargo install toni-cli --no-default-features` gives a scaffolding-only install without the watchexec dependency tree.

Deferred to a follow-up: gap-free restarts (holding the listening socket across rebuilds so connections queue instead of being refused) — that needs a from-listener path in the adapter SPI, independent of the CLI.
