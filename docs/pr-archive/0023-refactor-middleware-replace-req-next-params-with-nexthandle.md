# #23 —  refactor(middleware): replace (req, next) params with NextHandle

Merged 2026-03-28 into `master` from `refactor/middleware-next-handle`, commit [`271fed1`](https://github.com/ulo-rs/ulo/commit/271fed15cb97ace2977a05a421cde46873c71aa1).

## Summary

- Replaces `handle(&self, req: HttpRequest, next: Box<dyn Next>)` with `handle(&self, next: NextHandle)` — middlewares that don't need the request no longer have to accept it
- `NextHandle` bundles the in-flight request and its continuation: `next.request()`, `next.request_mut()`, `next.run()`, `next.run_with(req)`
- `Next` trait is now `NextInternal` (`pub(crate)`); users never reference it directly
- Also removes unused `_req: HttpRequest` parameters from controller handlers across tests, examples, READMEs, and CLI templates

## Test plan

- [ ] `cargo check --all-targets` passes with no errors
- [ ] Run integration tests: `cargo test -p integration-tests`
- [ ] Verify Tower compat still works: `cargo test -p integration-tests tower_compat`
- [ ] Verify middleware error propagation: `cargo test -p integration-tests middleware_error`
