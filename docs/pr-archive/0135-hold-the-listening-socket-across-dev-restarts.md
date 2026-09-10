# #135 — Hold the listening socket across dev restarts

Merged 2026-08-10 into `master` from `feat/cli-dev-socket-handoff`, commit [`0d2e2aa`](https://github.com/ulo-rs/ulo/commit/0d2e2aa2859caa7cae0ff2ed2e3fe16e06ecb188).

`toni dev --listen <ADDR>` binds the listening socket in the watcher and passes it to every restart, so requests arriving during a rebuild wait in the kernel accept queue and are answered by the new process instead of being refused.

- **CLI** — `--listen` binds the socket and installs a watchexec spawn hook that places it at descriptor 3 for each spawned process, announced through the systemd socket-activation variables (`LISTEN_FDS`, `LISTEN_FDS_FIRST_FD`). `LISTEN_PID` is left unset so the pid check is skipped: the application is a grandchild of the watcher via `cargo run`, so no pid known at spawn time could match the process that claims the socket. Unix only — the flag errors elsewhere, since the mechanism is file-descriptor passing.
- **Tests** — cover the descriptor handed to each restart, including the case where the socket already occupies descriptor 3, where `dup2` reports success without clearing the close-on-exec flag and the socket would be closed at exec.
- **Example** — `socket_activation` shows the adoption half: take the inherited listener when there is one, bind normally when there is not, so the binary still runs standalone. This is the same code production socket activation under systemd needs.

The adapter-side half already exists: `BindTarget::Listener` and `From<TcpListener>` landed in #134, so nothing in core or the adapters changes here. The end-to-end contract — a client connecting between two generations is answered rather than refused — is pinned by `bind_target_handoff.rs` from that PR.

Rocket is unsupported, as it is for any pre-bound listener — it binds from its own figment configuration and refuses one at `app.bind()`.
