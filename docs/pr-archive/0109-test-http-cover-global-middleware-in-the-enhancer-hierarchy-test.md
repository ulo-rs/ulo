# #109 — test(http): cover global middleware in the enhancer hierarchy test

Merged 2026-06-20 into `master` from `test/global-middleware-coverage`, commit [`0adac0e`](https://github.com/ulo-rs/ulo/commit/0adac0e8e8d55577abe7f14ccff9fb391fca8b78).

Global middleware had no end-to-end coverage — only a unit test asserting the manager's vec length. The global enhancer hierarchy test in `global_enhancers.rs` exercised guards, interceptors, and pipes but omitted middleware, leaving the outermost request layer unverified.

This folds a `GlobalMiddleware` into that test, registered via `factory.use_global_middleware(...)`, and asserts it wraps the entire enhancer pipeline: its `:before` is the first event (ahead of `guard:global`) and its `:after` is the last, after the pipeline unwinds. The ordering is checked across all three existing sub-cases (three-level, two-level, duplicate).
