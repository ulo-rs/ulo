# #188 — Let toni dev choose the cargo target and features

Merged 2026-08-24 into `master` from `feat/dev-forwards-cargo-flags`, commit [`0936199`](https://github.com/ulo-rs/ulo/commit/0936199ee4cff15e7a8fec77f6b9f5b509712450).

`toni dev` builds and runs whatever the mirrored cargo flags select, instead of always the package default binary. `--` still marks the boundary: everything before it is cargo's, everything after it is the application's.

- Target selection: `-p/--package`, `--bin`, `--example`. `--bin` and `--example` together are refused at parse time rather than by cargo on every restart.
- Feature selection: `-F/--features` (repeatable), `--all-features`, `--no-default-features`.
- `--cargo-arg <ARG>`, repeatable, forwards an argument to cargo verbatim, for flags this command does not mirror.

Tests pin the assembled invocation: each flag to the cargo flag it stands for, and every selector ahead of the `--`. That ordering is the one worth pinning — a selector emitted after the separator reaches the application binary, which then rejects an argument it was never meant to see.

`--profile` is not mirrored, since `--release` already covers the case the command was built for; `--cargo-arg --profile <name>` reaches cargo.
