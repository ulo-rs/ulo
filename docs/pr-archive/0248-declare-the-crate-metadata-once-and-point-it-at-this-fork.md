# #248 — Declare the crate metadata once, and point it at this fork

Merged 2026-09-10 into `master` from `chore/point-crate-metadata-at-this-fork`, commit [`b9137b2`](https://github.com/ulo-rs/ulo/commit/b9137b262b0d4aab860562274487e610daf0fc78).

Every crate's published metadata now comes from one place, and the workspace declares its shared dependency versions once.

- **Links** — `repository` and `homepage` point at this fork. Eighteen crates carried neither and would have published with no route back to the source.
- **Inheritance** — `edition`, `license`, `repository` and `homepage` come from `[workspace.package]`; twenty-two shared dependencies come from `[workspace.dependencies]`.
- **Edition** — editions were split eighteen to thirteen, leaving the test crates on Rust 2021 while the framework crate compiled under 2024. On 2024 they meet the rules a 2024 user's code meets, which took an `unsafe` block around each `std::env` mutation and a `use<>` bound on the two SSE handlers. That bound is what a user on edition 2024 writes today: `Sse`'s `IntoResponse` needs a `'static` stream, and an `impl Stream` returned from `&self` captures the borrow.
- **Features** — unchanged for every crate. `tokio` and `tower` declare `default-features = false`, a no-op for both, neither having a default feature. `futures` turns defaults off to match the ten members that already did, and the six that relied on them name `async-await` and `executor`.
- **Formatting** — `cargo fmt --all` output under the 2024 style edition.
- **CI** — `cargo test --workspace --all-features --doc` joins the test job. Doc examples compile in no other gate: `--all-targets` skips them and the docs job renders without running them.

Deferred to a follow-up:

- `reqwest` keeps its per-crate spelling: it is split across 0.12 and 0.13.
- No `rust-version`: the floors differ per crate.
