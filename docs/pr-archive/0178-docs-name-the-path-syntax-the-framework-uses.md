# #178 — docs: name the path syntax the framework uses

Merged 2026-08-23 into `master` from `docs/param-syntax-names-what-toni-uses`, commit [`7f7ddd4`](https://github.com/ulo-rs/ulo/commit/7f7ddd4dff8c26f75dd629bb7ae04ca2d71a1da4).

The axum, actix, and salvo adapters each carry a doc comment describing `:param` as toni's path syntax. `{param}` has been canonical since ADR-0012, and the route macros reject `:param` with a migration error, so a handler written against the framework can never produce a `:param` path.

The `to_*_path` rewrite functions are unchanged. They stay reachable for callers registering routes through the adapter SPI directly, which ADR-0012 kept deliberately lenient. The doc comments describe that, rather than presenting the rewrite as a translation from the framework's own syntax.

Prose only; no behavior change.
