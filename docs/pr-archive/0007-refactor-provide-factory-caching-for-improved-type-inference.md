# #7 — Refactor provide_factory! caching for improved type inference

Merged 2026-03-22 into `master` from `refactor/factory-generic-type-inference`, commit [`d2ced77`](https://github.com/ulo-rs/ulo/commit/d2ced77273b38684878eed57bf798770f1fbf04e).

Unify the caching mechanism for `provide_factory!` into a single generic struct, allowing the compiler to infer types without requiring explicit annotations. This change enhances usability and maintains backwards compatibility by silently discarding existing type hints in user code.
