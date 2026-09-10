# #38 — refactor: remove role accessor methods from Provider trait

Merged 2026-04-18 into `master` from `refactor/provider-role-accessors`, commit [`f786200`](https://github.com/ulo-rs/ulo/commit/f7862005ac1b318317701eb869abb0b317bd5b3f).

as_guard, as_interceptor, as_pipe, as_middleware, as_error_handler, as_gateway, and as_rpc_controller cluttered the Provider trait with default no-op methods that most implementors never touch.
