# #105 — chore(fmt): rustfmt the workspace + add a fmt CI check

Merged 2026-06-18 into `master` from `chore/fmt-baseline-ci`, commit [`083f575`](https://github.com/ulo-rs/ulo/commit/083f575e7036d59538841a716f408c5a53f1b810).

Formats the entire workspace with `rustfmt` and adds a CI check that keeps it that way.

The tree had accumulated formatting drift — mod ordering, import wrapping, long lines — so any `cargo fmt` swept files unrelated to the change at hand, which is why formatting had to be done by hand. A one-time `cargo fmt --all` makes the tree canonical: from here, formatting a touched file produces only that file's diff. A GitHub Actions `rustfmt` job (`cargo fmt --all --check`, on push and PR) enforces it.

Whitespace and layout only — no behavior change; `cargo build --workspace` is clean and the 232 integration tests pass. The large diff is entirely mechanical `rustfmt` output; the one hand-written file is `.github/workflows/ci.yml`. `cargo fmt` is edition-aware, so the mixed 2021/2024 crates format correctly per crate.

Build, test, and clippy jobs can be added to the workflow later — this is scoped to formatting.
