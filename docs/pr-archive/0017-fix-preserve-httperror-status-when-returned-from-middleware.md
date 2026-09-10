# #17 — fix: preserve HttpError status when returned from middleware

Merged 2026-03-27 into `master` from `feat/toni-error`, commit [`6ecd79f`](https://github.com/ulo-rs/ulo/commit/6ecd79ff3c2e93412403994672bc9ad8a56b26d2).

## Summary

Middleware returning `Err(HttpError::unauthorized(...))` was always producing a 500 response. The error recovery path in `InstanceWrapper` re-boxed the error as `io::Error`, losing the original type before checking error handlers or building the fallback response.

The fix downcasts to `HttpError` first and uses its intended status and body directly, so `401`, `403`, `429`, etc. reach the client as intended.

`HttpError` is also now re-exported from the `toni` crate root — no need to import from `toni::errors::HttpError`.

## Test plan

- [ ] `cargo test --package integration-tests --test middleware_error`
