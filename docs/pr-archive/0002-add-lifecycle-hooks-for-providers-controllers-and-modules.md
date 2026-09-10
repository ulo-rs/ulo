# #2 — Add lifecycle hooks for providers, controllers, and modules

Merged 2026-03-02 into `master` from `feat/lifecycle-hooks`, commit [`85ce44b`](https://github.com/ulo-rs/ulo/commit/85ce44b1846c45bd8e486f42a94a47fb1c1fb8a5).

Introduce lifecycle hooks to enable initialization and cleanup logic for providers, controllers, and modules, facilitating graceful application shutdown and startup processes. Extend the HttpAdapter with a default close method and refactor the application context for better dependency injection management.
